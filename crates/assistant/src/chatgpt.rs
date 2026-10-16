//! Signing in with ChatGPT, so the user's ChatGPT plan pays for the assistant instead of an API
//! key: OpenAI's Sign in with ChatGPT for open-source apps
//! (<https://developers.openai.com/siwc/token-sharing-open-source>).
//!
//! The first sign-in registers RustGIS as the user's own agent (`client_id=dynamic_agent_client`,
//! with this computer's host id); the browser comes back with the client id OpenAI issued, which
//! later sign-ins and renewals use. It is OAuth 2.0 authorization code with PKCE in the system
//! browser, returning to a short-lived server on `127.0.0.1`. The id token is verified against
//! OpenAI's published keys, and the grant must include `chatgpt.tokens.use.direct`, the
//! permission to use the plan. The access token is a bearer token for the public Responses API
//! (`https://api.openai.com/v1`), renewed with its rotating refresh token, and revoked on sign-out.
//!
//! The pure parts (the authorization page, the token requests and replies, the credentials file's
//! shape, deciding when to renew) are plain data for the tests; the files, the id token's
//! signature and the callback server are desktop only. Tokens are never logged, and the `Debug`
//! form of [`Credentials`] hides them.

use base64::Engine;
use serde_json::{Value, json};

use crate::provider::{HttpRequest, HttpResponse};

/// The OpenAI sign-in service (the id token's issuer).
pub const ISSUER: &str = "https://auth.openai.com";
/// Where the browser signs in.
pub const AUTHORIZE_URL: &str = "https://auth.openai.com/api/accounts/authorize";
/// Where codes and refresh tokens are exchanged.
pub const TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";
/// OpenAI's OpenID configuration: its signing keys and revocation endpoint.
pub const DISCOVERY_URL: &str = "https://auth.openai.com/.well-known/openid-configuration";
/// The API the plan's tokens are for (the `resource` of every grant).
pub const API: &str = "https://api.openai.com/v1";
/// The client id of a first sign-in, which registers RustGIS for the account.
pub const REGISTER: &str = "dynamic_agent_client";
/// The name the registration suggests (the user may edit it).
pub const AGENT_NAME: &str = "RustGIS";
/// Identity, a refresh token, and the permission to use the plan.
pub const SCOPES: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// The permission to use the user's ChatGPT plan.
pub const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
/// The callback port tried first (another free one otherwise).
pub const PORT: u16 = 1455;
/// Where the user reviews and limits the plan's usage.
pub const USAGE_URL: &str = "https://chatgpt.com/settings/usage?tab=overview";
/// Access tokens are renewed this long before they expire.
pub const REFRESH_MARGIN: i64 = 5 * 60;
/// The clock skew allowed when checking an id token's expiry.
pub const CLOCK_SKEW: i64 = 5 * 60;

/// The registration of RustGIS for a ChatGPT account and, while signed in, its tokens.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Credentials {
    /// The client id OpenAI issued for this account (kept after signing out).
    pub client_id: String,
    /// The verified id token's subject: who signed in.
    pub subject: String,
    /// The verified email, for display.
    pub email: String,
    /// The retained identity token.
    pub id_token: String,
    /// The bearer credential.
    pub access_token: String,
    /// The rotating renewal credential.
    pub refresh_token: String,
    /// When the access token expires (Unix seconds; 0 when unknown).
    pub expires_at: i64,
    /// The scopes granted.
    pub scopes: Vec<String>,
    /// The user declined the plan's use: the next sign-in asks for it again.
    pub plan_declined: bool,
    /// When the tokens were obtained (RFC 3339, informational).
    pub saved_at: String,
    /// Changes with each sign-in and sign-out (not renewal or selection): work begun before
    /// either is refused after it.
    pub session: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("client_id", &self.client_id)
            .field("signed_in", &self.signed_in())
            .field("scopes", &self.scopes)
            .field("saved_at", &self.saved_at)
            .finish_non_exhaustive()
    }
}

/// Who is signed in, for the settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Account {
    /// The verified email, for display.
    pub email: String,
    /// The id token's subject.
    pub subject: String,
}

/// The registration a request is for: its issued client id, the verified subject, and the
/// sign-in session it began in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Binding {
    pub client_id: String,
    pub subject: String,
    pub session: String,
}

impl Credentials {
    /// The registration these credentials are for.
    pub fn binding(&self) -> Binding {
        Binding { client_id: self.client_id.clone(), subject: self.subject.clone(), session: self.session.clone() }
    }

    /// Whether there are tokens to send.
    pub fn signed_in(&self) -> bool {
        !self.access_token.is_empty()
    }

    /// Whether the grant lets RustGIS use the plan.
    pub fn plan_allowed(&self) -> bool {
        self.scopes.iter().any(|s| s == PLAN_SCOPE)
    }

    /// The registration alone: what a sign-out keeps for the next sign-in.
    pub fn registration(&self) -> Credentials {
        Credentials {
            client_id: self.client_id.clone(),
            subject: self.subject.clone(),
            email: self.email.clone(),
            plan_declined: self.plan_declined,
            session: self.session.clone(),
            ..Default::default()
        }
    }

    /// A stable label that distinguishes registrations with the same email.
    pub fn label(&self) -> String {
        let who = if self.email.is_empty() { &self.subject } else { &self.email };
        format!("{who} · {}", self.client_id)
    }

    /// The verified identity for display.
    pub fn account(&self) -> Account {
        Account { email: self.email.clone(), subject: self.subject.clone() }
    }

    /// Reads a saved registration.
    pub fn from_json(v: &Value) -> Result<Credentials, String> {
        let field = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let c = Credentials {
            client_id: field("client_id"),
            subject: field("subject"),
            email: field("email"),
            id_token: field("id_token"),
            access_token: field("access_token"),
            refresh_token: field("refresh_token"),
            expires_at: v.get("expires_at").and_then(Value::as_i64).unwrap_or(0),
            scopes: v.get("scopes").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect(),
            plan_declined: v.get("plan_declined").and_then(Value::as_bool).unwrap_or(false),
            saved_at: field("saved_at"),
            session: field("session"),
        };
        if c.client_id.is_empty() || c.client_id == REGISTER {
            return Err("the sign-in has no registration".into());
        }
        Ok(c)
    }

    /// Serializes credentials; never log this value.
    pub fn to_json(&self) -> Value {
        json!({
            "issuer": ISSUER,
            "client_id": self.client_id,
            "subject": self.subject,
            "email": self.email,
            "id_token": self.id_token,
            "access_token": self.access_token,
            "refresh_token": self.refresh_token,
            "token_type": "Bearer",
            "expires_at": self.expires_at,
            "scopes": self.scopes,
            "plan_declined": self.plan_declined,
            "saved_at": self.saved_at,
            "session": self.session,
        })
    }

    /// Whether the access token expires within [`REFRESH_MARGIN`] of `now` (Unix seconds), or
    /// when that isn't known.
    pub fn expires_soon(&self, now: i64) -> bool {
        self.expires_at <= 0 || self.expires_at.saturating_sub(REFRESH_MARGIN) <= now
    }

    /// Puts the access token on a request (replacing any credentials there).
    pub fn authorize(&self, mut req: HttpRequest) -> HttpRequest {
        req.headers.retain(|(k, _)| !k.eq_ignore_ascii_case("authorization"));
        req.headers.push(("Authorization".to_string(), format!("Bearer {}", self.access_token)));
        req
    }
}

/// The payload of a JWT, unverified.
pub(crate) fn jwt_payload(jwt: &str) -> Option<Value> {
    let mut parts = jwt.split('.');
    let (h, p, s) = (parts.next()?, parts.next()?, parts.next()?);
    if h.is_empty() || p.is_empty() || s.is_empty() || parts.next().is_some() {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(p).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Checks an id token's claims (its signature is checked separately): the issuer, the audience
/// (the issued client id), the expiry and the nonce of this sign-in. Gives the subject and email.
pub fn check_id_claims(claims: &Value, client_id: &str, nonce: &str, now: i64) -> Result<Account, String> {
    let s = |k: &str| claims.get(k).and_then(Value::as_str).unwrap_or("");
    if s("iss") != ISSUER {
        return Err("the id token isn't from OpenAI".into());
    }
    let aud_ok = match claims.get("aud") {
        Some(Value::String(a)) => a == client_id,
        Some(Value::Array(a)) => a.iter().any(|x| x.as_str() == Some(client_id)),
        _ => false,
    };
    if client_id.is_empty() || !aud_ok {
        return Err("the id token is for another app".into());
    }
    if claims.get("exp").and_then(Value::as_i64).is_none_or(|exp| exp.saturating_add(CLOCK_SKEW) < now) {
        return Err("the id token has expired".into());
    }
    if nonce.is_empty() || s("nonce") != nonce {
        return Err("the id token isn't from this sign-in".into());
    }
    if s("sub").is_empty() {
        return Err("the id token names no account".into());
    }
    Ok(Account { email: s("email").to_string(), subject: s("sub").to_string() })
}

/// What a token endpoint gave.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Grant {
    /// The bearer credential.
    pub access_token: String,
    /// The rotating renewal credential.
    pub refresh_token: String,
    /// The retained identity token.
    pub id_token: String,
    /// Seconds until expiry.
    pub expires_in: i64,
    /// The scopes granted, when the reply says.
    pub scopes: Option<Vec<String>>,
}

impl std::fmt::Debug for Grant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Grant").field("expires_in", &self.expires_in).field("scopes", &self.scopes).finish_non_exhaustive()
    }
}

/// Why a token endpoint refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrantError {
    /// The refresh token or code can't be used again: sign in again.
    Unusable,
    /// Anything else (a message without credentials).
    Other(String),
}

impl GrantError {
    /// A credential-free diagnostic.
    pub fn message(&self, what: &str) -> String {
        match self {
            GrantError::Unusable => format!("{what}: the sign-in has expired or was revoked; sign in again"),
            GrantError::Other(why) => format!("{what}: {why}"),
        }
    }
}

/// A token endpoint's reply, or why it refused.
pub fn parse_grant(resp: &HttpResponse) -> Result<Grant, GrantError> {
    let v: Value = serde_json::from_slice(&resp.body).unwrap_or(Value::Null);
    if !(200..300).contains(&resp.status) {
        let code = v.get("error").and_then(|e| e.as_str().or_else(|| e.get("code").and_then(Value::as_str))).unwrap_or("");
        // OAuth responses are untrusted and may echo submitted credentials. Report only
        // recognized protocol codes, never raw descriptions or response bodies.
        return Err(match code {
            "invalid_grant"
            | "invalid_refresh_token"
            | "token_expired"
            | "refresh_token_expired"
            | "refresh_token_invalidated"
            | "refresh_token_reused" => GrantError::Unusable,
            "invalid_request" | "invalid_client" | "unauthorized_client" | "unsupported_grant_type" | "invalid_scope" => {
                GrantError::Other(code.to_string())
            }
            _ => GrantError::Other(format!("HTTP {}", resp.status)),
        });
    }
    let field = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let g = Grant {
        access_token: field("access_token"),
        refresh_token: field("refresh_token"),
        id_token: field("id_token"),
        expires_in: v.get("expires_in").and_then(Value::as_i64).unwrap_or(0),
        scopes: v.get("scope").and_then(Value::as_str).map(|s| s.split_whitespace().map(str::to_string).collect()),
    };
    if g.access_token.is_empty() {
        return Err(GrantError::Other("the reply has no access token".into()));
    }
    Ok(g)
}

/// When a grant's access token expires: from `expires_in`, else the token's own `exp`.
fn expiry(g: &Grant, now: i64) -> i64 {
    if g.expires_in > 0 {
        return now.saturating_add(g.expires_in);
    }
    jwt_payload(&g.access_token).and_then(|v| v.get("exp").and_then(Value::as_i64)).unwrap_or(0)
}

/// The credentials of a completed sign-in: the registration's client id, the verified account,
/// and the grant.
pub fn signed_in(client_id: &str, account: &Account, g: &Grant, now: i64) -> Credentials {
    let scopes = g.scopes.clone().unwrap_or_default();
    Credentials {
        client_id: client_id.to_string(),
        subject: account.subject.clone(),
        email: account.email.clone(),
        id_token: g.id_token.clone(),
        access_token: g.access_token.clone(),
        refresh_token: g.refresh_token.clone(),
        expires_at: expiry(g, now),
        plan_declined: !scopes.iter().any(|s| s == PLAN_SCOPE),
        scopes,
        saved_at: rfc3339(now),
        session: String::new(),
    }
}

/// The credentials after a renewal: the access token, expiry, scopes and rotated refresh token
/// replaced together.
pub fn renewed(old: &Credentials, g: &Grant, now: i64) -> Credentials {
    let mut c = old.clone();
    c.access_token = g.access_token.clone();
    if !g.refresh_token.is_empty() {
        c.refresh_token = g.refresh_token.clone();
    }
    if !g.id_token.is_empty() {
        c.id_token = g.id_token.clone();
    }
    if let Some(s) = &g.scopes {
        c.scopes = s.clone();
        c.plan_declined = !c.plan_allowed();
    }
    c.expires_at = expiry(g, now);
    c.saved_at = rfc3339(now);
    c
}

/// Percent-encodes a query or form value.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn encode(params: &[(&str, &str)]) -> String {
    params.iter().map(|(k, v)| format!("{k}={}", urlencode(v))).collect::<Vec<_>>().join("&")
}

fn form_post(url: &str, params: &[(&str, &str)]) -> HttpRequest {
    HttpRequest {
        method: "POST",
        url: url.to_string(),
        headers: vec![("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string())],
        body: Some(encode(params)),
    }
}

/// Decodes a query value (`+` is a space).
pub fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes.get(i) {
            Some(b'+') => out.push(b' '),
            Some(b'%') => {
                let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(b) => {
                        out.push(b);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            Some(b) => out.push(*b),
            None => break,
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The (decoded) parameters of a query string.
pub fn query_params(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| p.split_once('=').unwrap_or((p, "")))
        .map(|(k, v)| (urldecode(k), urldecode(v)))
        .collect()
}

/// Only the Responses API and the model list may receive the plan's access token.
pub fn api_url(url: &str) -> bool {
    let Some(rest) = url.trim().strip_prefix(API) else { return false };
    let path = rest.split('?').next().unwrap_or("");
    (path == "/responses" || path == "/models") && !url.contains(['\\', '#', '@'])
}

#[cfg(any(test, not(target_arch = "wasm32")))]
pub(crate) fn callback_state_matches(params: &[(String, String)], expected: &str) -> bool {
    let mut states = params.iter().filter(|(key, _)| key == "state");
    !expected.is_empty() && states.next().is_some_and(|(_, value)| value == expected) && states.next().is_none()
}

/// The client id a callback gives for a sign-in started with `requested`: the issued one for a
/// first sign-in (which must say it), the saved one otherwise (which it may repeat, never change).
pub fn callback_client_id(params: &[(String, String)], requested: &str) -> Result<String, String> {
    let mut ids = params.iter().filter(|(k, _)| k == "client_id").map(|(_, v)| v.trim());
    let (given, extra) = (ids.next().filter(|v| !v.is_empty()), ids.next());
    if extra.is_some() {
        return Err("the sign-in returned more than one client id".into());
    }
    match (requested == REGISTER, given) {
        (true, Some(id)) if id != REGISTER => Ok(id.to_string()),
        (true, _) => Err("the registration didn't complete; try again".into()),
        (false, Some(id)) if id != requested => Err("the sign-in returned another registration; try again".into()),
        (false, _) => Ok(requested.to_string()),
    }
}

/// The PKCE challenge (S256) of a verifier.
pub fn pkce_challenge(verifier: &str) -> String {
    use sha2::Digest;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()))
}

/// A version 4 UUID from 16 random bytes, as a host id (`urn:uuid:…`).
pub fn host_id_from(b: [u8; 16]) -> String {
    let mut id = String::from("urn:uuid:");
    for (i, x) in b.iter().enumerate() {
        // The version (4) and variant (RFC 9562) bits.
        let x = match i {
            6 => (x & 0x0f) | 0x40,
            8 => (x & 0x3f) | 0x80,
            _ => *x,
        };
        if matches!(i, 4 | 6 | 8 | 10) {
            id.push('-');
        }
        id.push_str(&format!("{x:02x}"));
    }
    id
}

/// Whether a saved host id has the shape this app writes.
pub fn valid_host_id(id: &str) -> bool {
    id.strip_prefix("urn:uuid:").is_some_and(|u| {
        u.len() == 36 && u.char_indices().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { c == '-' } else { c.is_ascii_hexdigit() })
    })
}

/// One attempt at signing in.
pub struct Authorization<'a> {
    /// [`REGISTER`] for a first sign-in, else the account's issued client id.
    pub client_id: &'a str,
    /// The stable identity of this computer.
    pub host_id: &'a str,
    /// The bound loopback callback.
    pub redirect_uri: &'a str,
    /// The PKCE S256 challenge.
    pub challenge: &'a str,
    /// The random callback binding.
    pub state: &'a str,
    /// The random identity-token binding.
    pub nonce: &'a str,
    /// The account's email, to preselect it.
    pub login_hint: &'a str,
    /// The account's last id token, to skip choosing the account again (never after a sign-out;
    /// keep URLs carrying it out of logs, terminals and clipboards).
    pub id_token_hint: &'a str,
    /// Ask again for the permissions (after the plan's use was declined).
    pub consent: bool,
}

/// The page to open in the browser.
pub fn authorize_url(a: &Authorization) -> String {
    let mut params = vec![
        ("response_type", "code"),
        ("client_id", a.client_id),
        ("redirect_uri", a.redirect_uri),
        ("scope", SCOPES),
        ("resource", API),
        ("state", a.state),
        ("nonce", a.nonce),
        ("code_challenge", a.challenge),
        ("code_challenge_method", "S256"),
        ("ext_agent_host_id", a.host_id),
    ];
    if a.client_id == REGISTER {
        params.push(("agent_name_hint", AGENT_NAME));
    } else {
        if !a.id_token_hint.is_empty() {
            params.push(("id_token_hint", a.id_token_hint));
        }
        if !a.login_hint.is_empty() {
            params.push(("login_hint", a.login_hint));
        }
    }
    if a.consent {
        params.push(("prompt", "consent"));
    }
    format!("{AUTHORIZE_URL}?{}", encode(&params))
}

/// The request exchanging the browser's code for tokens.
pub fn exchange_request(client_id: &str, code: &str, redirect_uri: &str, verifier: &str) -> HttpRequest {
    form_post(
        TOKEN_URL,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect_uri),
            ("resource", API),
        ],
    )
}

/// The request renewing the tokens (the scope is left out to keep the grant).
pub fn refresh_request(c: &Credentials) -> HttpRequest {
    form_post(
        TOKEN_URL,
        &[("grant_type", "refresh_token"), ("client_id", &c.client_id), ("refresh_token", &c.refresh_token), ("resource", API)],
    )
}

/// The request revoking a refresh token at `endpoint` (from the OpenID configuration).
pub fn revoke_request(endpoint: &str, client_id: &str, refresh_token: &str) -> HttpRequest {
    form_post(endpoint, &[("token", refresh_token), ("token_type_hint", "refresh_token"), ("client_id", client_id)])
}

/// An endpoint of OpenAI's OpenID configuration, if it is on the sign-in service.
pub fn discovered(config: &Value, key: &str) -> Option<String> {
    config.get(key).and_then(Value::as_str).filter(|u| u.strip_prefix(ISSUER).is_some_and(|r| r.starts_with('/'))).map(str::to_string)
}

/// `secs` since the Unix epoch as RFC 3339 (`2026-10-14T13:34:16Z`).
pub fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil date from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

// ---------------------------------------------------------------------------------------------
// The desktop and the command line: the files, the id token's signature, renewing, signing in
// and out.

#[cfg(not(target_arch = "wasm32"))]
pub use native::*;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use base64::Engine;
    use serde_json::Value;

    use super::{
        Account, Authorization, Binding, Credentials, DISCOVERY_URL, GrantError, PORT, REGISTER, authorize_url, callback_client_id,
        check_id_claims, discovered, exchange_request, host_id_from, jwt_payload, parse_grant, query_params, refresh_request, renewed,
        revoke_request, valid_host_id,
    };
    use crate::provider::{HttpRequest, HttpResponse};
    use crate::{Error, Recovery, http};

    /// The credentials' file name, in the app's settings folder.
    const FILE: &str = "chatgpt-auth.json";
    /// This computer's host id, kept apart from the credentials so signing out keeps it.
    const HOST_FILE: &str = "chatgpt-host-id";

    /// How long the browser has to come back.
    const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

    type Send<'a> = &'a mut dyn FnMut(&HttpRequest) -> Result<HttpResponse, Error>;

    /// The settings folder the desktop app keeps its preferences in.
    pub fn settings_dir() -> Option<PathBuf> {
        if cfg!(target_os = "windows") {
            std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("RustGIS"))
        } else if cfg!(target_os = "macos") {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/RustGIS"))
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
                .map(|d| d.join("rustgis"))
        }
    }

    /// Where the sign-in is kept.
    pub fn auth_path() -> Option<PathBuf> {
        settings_dir().map(|d| d.join(FILE))
    }

    fn now() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
    }

    // Store locks cover only file transactions. Renewal locks may cover network I/O.
    fn file_lock(path: &Path, extension: &str) -> Result<std::fs::File, Error> {
        let dir = path.parent().ok_or("no settings folder")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension(extension))
            .map_err(|e| e.to_string())?;
        file.lock().map_err(|e| e.to_string())?;
        Ok(file)
    }

    fn store(path: &Path) -> Result<std::fs::File, Error> {
        file_lock(path, "store-lock")
    }

    #[derive(Default)]
    struct Accounts {
        entries: Vec<Credentials>,
        active: Option<(String, String)>,
    }

    impl Accounts {
        fn selected(&self) -> Option<&Credentials> {
            self.entries.iter().find(|c| self.active.as_ref() == Some(&(c.client_id.clone(), c.subject.clone())))
        }

        fn put(&mut self, c: &Credentials) {
            self.entries.retain(|old| old.client_id != c.client_id || old.subject != c.subject);
            self.entries.push(c.clone());
            self.active = Some((c.client_id.clone(), c.subject.clone()));
        }
    }

    // Callers hold the store lock for the entire read/compare/write transaction.
    fn read_accounts(path: &Path) -> Result<Accounts, Error> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Accounts::default()),
            Err(e) => return Err(e.to_string().into()),
        };
        let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|_| "the sign-in store isn't JSON")?;
        if let Some(entries) = v.get("accounts").and_then(Value::as_array) {
            let entries = entries.iter().map(Credentials::from_json).collect::<Result<Vec<_>, _>>()?;
            let active =
                v.get("active").and_then(|a| Some((a.get("client_id")?.as_str()?.to_string(), a.get("subject")?.as_str()?.to_string())));
            Ok(Accounts { entries, active })
        } else {
            let mut accounts = Accounts::default();
            accounts.put(&Credentials::from_json(&v)?);
            Ok(accounts)
        }
    }

    fn write_accounts(path: &Path, accounts: &Accounts) -> Result<(), Error> {
        let active = accounts.active.as_ref().map(|(client, subject)| serde_json::json!({"client_id": client, "subject": subject}));
        write_private(
            path,
            &serde_json::json!({"version": 2, "active": active,
            "accounts": accounts.entries.iter().map(Credentials::to_json).collect::<Vec<_>>()})
            .to_string(),
        )
    }

    fn read_credentials(path: &Path) -> Result<Credentials, Error> {
        read_accounts(path)?.selected().cloned().ok_or_else(|| "not signed in to ChatGPT".into())
    }

    /// The selected registration, including signed-out accounts.
    pub fn load_any() -> Option<Credentials> {
        let path = auth_path()?;
        let _guard = store(&path).ok()?;
        read_credentials(&path).ok()
    }

    /// Saved registrations, including those signed out.
    pub fn accounts() -> Result<Vec<Credentials>, Error> {
        let path = auth_path().ok_or("no settings folder")?;
        let _guard = store(&path)?;
        Ok(read_accounts(&path)?.entries)
    }

    /// Selects an existing registration without signing it in.
    pub fn select(client_id: &str, subject: &str) -> Result<Credentials, Error> {
        select_at(&auth_path().ok_or("no settings folder")?, client_id, subject)
    }

    fn select_at(path: &Path, client_id: &str, subject: &str) -> Result<Credentials, Error> {
        let _guard = store(path)?;
        let mut accounts = read_accounts(path)?;
        accounts.active = Some((client_id.to_string(), subject.to_string()));
        let c = accounts.selected().cloned().ok_or("the saved account wasn't found")?;
        write_accounts(path, &accounts)?;
        Ok(c)
    }

    /// The account signed in on this computer with the plan's use allowed, if any.
    pub fn load() -> Option<Credentials> {
        load_any().filter(|c| c.signed_in() && c.plan_allowed())
    }

    /// The account signed in, as its verified id token named it.
    pub fn account() -> Option<Account> {
        load().map(|c| c.account())
    }

    /// Writes `text` to `path` atomically, readable by this user only.
    fn write_private(path: &Path, text: &str) -> Result<(), Error> {
        let dir = path.parent().ok_or("no settings folder")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        // A unique, exclusively created file avoids races and pre-existing symlinks. Tempfiles
        // have owner-only Unix permissions (Windows keeps the settings folder's own access
        // list); persist replaces atomically, including on Windows.
        let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        tmp.write_all(text.as_bytes()).and_then(|_| tmp.as_file().sync_all()).map_err(|e| format!("{}: {e}", path.display()))?;
        tmp.persist(path).map_err(|e| format!("{}: {}", path.display(), e.error))?;
        Ok(())
    }

    fn write_credentials(path: &Path, c: &Credentials) -> Result<(), Error> {
        let mut accounts = read_accounts(path)?;
        accounts.put(c);
        write_accounts(path, &accounts)
    }

    /// This computer's host id, created the first time.
    pub fn host_id() -> Result<String, Error> {
        let path = settings_dir().ok_or("no settings folder")?.join(HOST_FILE);
        let _guard = store(&path)?;
        match std::fs::read_to_string(&path) {
            Ok(id) if valid_host_id(id.trim()) => return Ok(id.trim().to_string()),
            // Missing or malformed: a new one. Any other failure must not replace this host's id.
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display()).into()),
        }
        let mut b = [0u8; 16];
        getrandom::fill(&mut b).map_err(|e| format!("no random numbers: {e}"))?;
        let id = host_id_from(b);
        write_private(&path, &id)?;
        Ok(id)
    }

    /// Verifies an RS256 JWT's signature against a JWKS (`{"keys": […]}`) and gives its claims.
    pub fn verify_jwt(jwt: &str, jwks: &Value) -> Result<Value, Error> {
        let b64 = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s).ok();
        let mut parts = jwt.splitn(3, '.');
        let (h, p, s) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let header: Value = b64(h).and_then(|b| serde_json::from_slice(&b).ok()).ok_or("the id token is malformed")?;
        if header.get("alg").and_then(Value::as_str) != Some("RS256") {
            return Err("the id token isn't signed with RS256".into());
        }
        let kid = header.get("kid").and_then(Value::as_str);
        let key = jwks
            .get("keys")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|k| k.get("kty").and_then(Value::as_str) == Some("RSA"))
            .find(|k| kid.is_none_or(|kid| k.get("kid").and_then(Value::as_str) == Some(kid)))
            .ok_or("the id token's signing key isn't published")?;
        let n = key.get("n").and_then(Value::as_str).and_then(b64).ok_or("a signing key is malformed")?;
        let e = key.get("e").and_then(Value::as_str).and_then(b64).ok_or("a signing key is malformed")?;
        let sig = b64(s).ok_or("the id token is malformed")?;
        let signed = format!("{h}.{p}");
        ring::signature::RsaPublicKeyComponents { n: &n, e: &e }
            .verify(&ring::signature::RSA_PKCS1_2048_8192_SHA256, signed.as_bytes(), &sig)
            .map_err(|_| "the id token's signature is invalid")?;
        jwt_payload(jwt).ok_or_else(|| Error::auth("the id token is malformed".into()))
    }

    fn get_json(url: &str, send: Send) -> Result<Value, Error> {
        let r = send(&HttpRequest { method: "GET", url: url.to_string(), headers: vec![], body: None })?;
        if !(200..300).contains(&r.status) {
            return Err(Error {
                status: Some(r.status),
                request_id: r.request_id.clone(),
                ..Error::auth(format!("{url}: HTTP {}", r.status))
            });
        }
        serde_json::from_slice(&r.body).map_err(|_| Error::auth(format!("{url}: not JSON")))
    }

    /// Verifies a new id token with OpenAI's published keys.
    fn verify_id_token(id_token: &str, client_id: &str, nonce: &str, send: Send) -> Result<Account, Error> {
        let config = get_json(DISCOVERY_URL, send)?;
        let jwks_uri = discovered(&config, "jwks_uri").ok_or("OpenAI's sign-in configuration has no signing keys")?;
        let claims = verify_jwt(id_token, &get_json(&jwks_uri, send)?)?;
        check_id_claims(&claims, client_id, nonce, now()).map_err(Error::auth)
    }

    /// Renews the tokens with `send` and keeps the result. An unusable refresh token is cleared
    /// (the registration stays for the next sign-in).
    fn refresh(path: &Path, c: &Credentials, send: Send) -> Result<Credentials, Error> {
        if c.refresh_token.is_empty() {
            return Err(Error::new(Recovery::SignIn, "the sign-in can't be renewed; sign in again"));
        }
        let resp = send(&refresh_request(c))?;
        let grant = parse_grant(&resp);
        let _guard = store(path)?;
        // A user can sign out or sign into another account while the grant is in flight.
        if read_credentials(path).ok().as_ref() != Some(c) {
            return Err(Error::new(Recovery::AccountChanged, "the ChatGPT sign-in changed; try again"));
        }
        match grant {
            Ok(g) => {
                let fresh = renewed(c, &g, now());
                write_credentials(path, &fresh)?;
                Ok(fresh)
            }
            Err(GrantError::Unusable) => {
                // Not a sign-out: the id token stays to skip choosing the account next time.
                let mut registration = c.registration();
                registration.id_token = c.id_token.clone();
                write_credentials(path, &registration)?;
                Err(Error::token(&resp, GrantError::Unusable))
            }
            Err(e) => Err(Error::token(&resp, e)),
        }
    }

    fn renewal_lock(path: &Path) -> Result<std::fs::File, Error> {
        file_lock(path, "renewal-lock")
    }

    /// The signed-in account's credentials, renewed with `send` when about to expire.
    pub fn fresh(send: Send) -> Result<Credentials, Error> {
        let path = auth_path().ok_or("no settings folder")?;
        let _guard = renewal_lock(&path)?;
        fresh_at(&path, send)
    }

    fn fresh_at(path: &Path, send: Send) -> Result<Credentials, Error> {
        let c = {
            let _guard = store(path)?;
            read_credentials(path)
                .ok()
                .filter(Credentials::signed_in)
                .ok_or_else(|| Error::new(Recovery::SignIn, "not signed in to ChatGPT"))?
        };
        if !c.plan_allowed() {
            return Err(Error::new(Recovery::Consent, "RustGIS isn't allowed to use your ChatGPT plan; sign in again and allow it"));
        }
        if c.expires_soon(now()) { refresh(path, &c, send) } else { Ok(c) }
    }

    /// Sends a request as `account` (blocking): refused if another account is now selected or it
    /// signed out; the tokens are renewed first when about to expire, and once more if the API
    /// still refuses them.
    pub fn send(req: &HttpRequest, account: &Binding) -> Result<HttpResponse, Error> {
        send_inner(req, account).map_err(Error::auth_context)
    }

    fn send_inner(req: &HttpRequest, account: &Binding) -> Result<HttpResponse, Error> {
        // Reject an edited address before loading credentials or rotating any tokens.
        if !super::api_url(&req.url) {
            return Err(Error::new(Recovery::Configuration, "a ChatGPT sign-in can only be used with the OpenAI API"));
        }
        send_at(req, account, &auth_path().ok_or("no settings folder")?, &mut |req| {
            if req.url == super::TOKEN_URL {
                http::send_timeout(req, Duration::from_secs(30)).map_err(Error::auth)
            } else {
                http::send(req).map_err(Error::auth)
            }
        })
    }

    fn send_at(req: &HttpRequest, account: &Binding, path: &Path, transport: Send) -> Result<HttpResponse, Error> {
        let c = {
            let _guard = renewal_lock(path)?;
            {
                let _store = store(path)?;
                // No saved sign-in at all reads as signed out.
                check_account(account, &read_credentials(path).unwrap_or_default())?;
            }
            fresh_at(path, transport)?
        };
        check_final(path, account, &c)?;
        let resp = transport(&c.authorize(req.clone()))?;
        if resp.status != 401 {
            return Ok(resp);
        }
        let c = {
            let _guard = renewal_lock(path)?;
            let current = {
                let _store = store(path)?;
                read_credentials(path)
                    .ok()
                    .filter(Credentials::signed_in)
                    .ok_or_else(|| Error::new(Recovery::SignIn, "not signed in to ChatGPT"))?
            };
            // Another request may already have rotated this token while ours was in flight.
            // Signing out or into another account must not be undone by a late 401 either.
            if current.binding() != c.binding() {
                return Err(Error::new(Recovery::AccountChanged, "the ChatGPT account changed; try again"));
            }
            if current.access_token != c.access_token { current } else { refresh(path, &current, transport)? }
        };
        check_final(path, account, &c)?;
        transport(&c.authorize(req.clone()))
    }

    /// Refuses a request built for another account than the selected one, or after it signed out.
    fn check_account(account: &Binding, c: &Credentials) -> Result<(), Error> {
        if !c.signed_in() {
            return Err(Error::new(Recovery::SignIn, "not signed in to ChatGPT"));
        }
        if c.binding() != *account {
            return Err(Error::new(Recovery::AccountChanged, "the ChatGPT account changed; select the account again"));
        }
        Ok(())
    }

    fn check_final(path: &Path, account: &Binding, c: &Credentials) -> Result<(), Error> {
        let _guard = store(path)?;
        let saved = read_credentials(path)?;
        check_account(account, &saved)?;
        if !c.plan_allowed() || !saved.plan_allowed() {
            return Err(Error::new(Recovery::Consent, "RustGIS isn't allowed to use your ChatGPT plan; sign in again and allow it"));
        }
        Ok(())
    }

    // -----------------------------------------------------------------------------------------
    // Signing out

    /// What revoking a sign-out's session needs.
    pub struct Revocation {
        client_id: String,
        refresh_token: String,
    }

    impl std::fmt::Debug for Revocation {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Revocation").field("client_id", &self.client_id).finish_non_exhaustive()
        }
    }

    /// Signs out on this computer at once: the tokens are cleared (the registration and host id
    /// stay for the next sign-in). Revoke the session with what it gives ([`revoke`]), off the
    /// UI thread.
    pub fn sign_out() -> Result<Option<Revocation>, Error> {
        match auth_path() {
            Some(path) => sign_out_at(&path),
            None => Ok(None),
        }
    }

    /// Signs out only if the displayed registration is still selected.
    pub fn sign_out_selected(account: &Binding) -> Result<Option<Revocation>, Error> {
        let path = auth_path().ok_or("no settings folder")?;
        sign_out_checked(&path, Some(account))
    }

    fn sign_out_at(path: &Path) -> Result<Option<Revocation>, Error> {
        sign_out_checked(path, None)
    }

    fn sign_out_checked(path: &Path, expected: Option<&Binding>) -> Result<Option<Revocation>, Error> {
        let _guard = store(path)?;
        let accounts = read_accounts(path)?;
        let Some(c) = accounts.selected() else { return Ok(None) };
        if expected.is_some_and(|e| *e != c.binding()) {
            return Err(Error::new(Recovery::AccountChanged, "the ChatGPT sign-in changed; select the account again"));
        }
        let mut registration = c.registration();
        registration.session = random_token()?;
        write_credentials(path, &registration)?;
        Ok((!c.refresh_token.is_empty()).then_some(Revocation { client_id: c.client_id.clone(), refresh_token: c.refresh_token.clone() }))
    }

    /// Ends a signed-out session at OpenAI (blocking; retried a few times on network and server
    /// errors). An error means OpenAI didn't confirm it: the user can disconnect RustGIS in their
    /// ChatGPT settings.
    pub fn revoke(r: &Revocation) -> Result<(), Error> {
        revoke_with(r, &mut |req| http::send_timeout(req, Duration::from_secs(30)).map_err(Error::auth), &mut |d| std::thread::sleep(d))
    }

    fn revoke_with(r: &Revocation, send: Send, wait: &mut dyn FnMut(Duration)) -> Result<(), Error> {
        let mut endpoint = None;
        let mut last = Error::auth("revocation not confirmed".into());
        for attempt in 0..3u32 {
            if attempt > 0 {
                wait(Duration::from_secs(1 << attempt));
            }
            let url = match &endpoint {
                Some(url) => url,
                None => match get_json(DISCOVERY_URL, send) {
                    Ok(config) => {
                        let url = discovered(&config, "revocation_endpoint")
                            .ok_or("OpenAI's sign-in configuration has no revocation endpoint")?;
                        endpoint.insert(url)
                    }
                    Err(e) => {
                        last = e;
                        continue;
                    }
                },
            };
            match send(&revoke_request(url, &r.client_id, &r.refresh_token)) {
                Ok(resp) if (200..300).contains(&resp.status) => return Ok(()),
                Ok(resp) if resp.status < 500 => return Err(Error::auth(format!("revoking the sign-in: HTTP {}", resp.status))),
                Ok(resp) => last = Error::auth(format!("revoking the sign-in: HTTP {}", resp.status)),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    // -----------------------------------------------------------------------------------------
    // Signing in: the browser flow

    type Outcome = Arc<Mutex<Option<Result<Credentials, Error>>>>;

    /// A sign-in under way: open [`Login::url`] in the browser, then [`Login::poll`] until the
    /// credentials arrive. Dropping it stops the callback server.
    pub struct Login {
        /// The authorization page, safe to show or copy (without the id token hint).
        pub url: String,
        /// The same page with the id token hint, for opening the browser only.
        pub browser_url: String,
        /// The loopback listener port.
        pub port: u16,
        outcome: Outcome,
        cancel: Arc<Mutex<bool>>,
    }

    impl Login {
        /// The outcome, once there is one (taken: later polls say nothing).
        pub fn poll(&self) -> Option<Result<Credentials, Error>> {
            self.outcome.lock().ok().and_then(|mut o| o.take()).map(|r| r.map_err(Error::auth_context))
        }

        /// Stops the listener and discards any outstanding grant.
        pub fn cancel(&self) {
            if let Ok(mut cancel) = self.cancel.lock() {
                *cancel = true;
            }
        }
    }

    impl Drop for Login {
        fn drop(&mut self) {
            self.cancel();
        }
    }

    fn random_token() -> Result<String, Error> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| format!("no random numbers: {e}"))?;
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
    }

    /// What the callback server needs to finish a sign-in.
    struct Pending {
        redirect_uri: String,
        verifier: String,
        state: String,
        nonce: String,
        /// [`REGISTER`] or the saved client id.
        client_id: String,
        /// The account a sign-in with a saved registration must come back as.
        subject: String,
        expected: Option<Credentials>,
    }

    /// Starts a sign-in: binds the callback server and makes the page to open. The credentials
    /// are saved when they arrive. A sign-in uses the account registered on this computer, if
    /// any; `another_account` adds a separate registration.
    pub fn login(another_account: bool) -> Result<Login, Error> {
        login_inner(another_account).map_err(Error::auth_context)
    }

    fn login_inner(another_account: bool) -> Result<Login, Error> {
        let host_id = host_id()?;
        let expected = load_any();
        let saved = if another_account { None } else { expected.clone() };
        let (listener, port) = TcpListener::bind(("127.0.0.1", PORT))
            .or_else(|_| TcpListener::bind(("127.0.0.1", 0)))
            .and_then(|l| l.local_addr().map(|a| (l, a.port())))
            .map_err(|e| format!("can't start the sign-in server: {e}"))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let p = Pending {
            redirect_uri: format!("http://127.0.0.1:{port}/auth/callback"),
            verifier: random_token()?,
            state: random_token()?,
            nonce: random_token()?,
            client_id: saved.as_ref().map_or_else(|| REGISTER.to_string(), |c| c.client_id.clone()),
            subject: saved.as_ref().map(|c| c.subject.clone()).unwrap_or_default(),
            expected,
        };
        let challenge = super::pkce_challenge(&p.verifier);
        let page = |id_token_hint: &str| {
            authorize_url(&Authorization {
                client_id: &p.client_id,
                host_id: &host_id,
                redirect_uri: &p.redirect_uri,
                challenge: &challenge,
                state: &p.state,
                nonce: &p.nonce,
                login_hint: saved.as_ref().map_or("", |c| c.email.as_str()),
                id_token_hint,
                consent: saved.as_ref().is_some_and(|c| c.plan_declined),
            })
        };
        let (url, browser_url) = (page(""), page(saved.as_ref().map_or("", |c| c.id_token.as_str())));
        let outcome: Outcome = Arc::default();
        let cancel = Arc::new(Mutex::new(false));
        let (out, stop) = (outcome.clone(), cancel.clone());
        std::thread::spawn(move || {
            let result = serve(&listener, &stop, &p);
            if let Ok(mut o) = out.lock() {
                *o = Some(result);
            }
        });
        Ok(Login { url, browser_url, port, outcome, cancel })
    }

    /// Answers requests until the callback arrives (or the sign-in is cancelled).
    fn serve(listener: &TcpListener, cancel: &Arc<Mutex<bool>>, p: &Pending) -> Result<Credentials, Error> {
        let deadline = Instant::now() + LOGIN_TIMEOUT;
        loop {
            check_attempt(cancel, deadline)?;
            let mut stream = match listener.accept() {
                Ok((s, _)) => s,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted | std::io::ErrorKind::ConnectionAborted
                    ) =>
                {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                Err(e) => return Err(Error::auth(format!("the sign-in server failed: {e}"))),
            };
            let _ = stream.set_nonblocking(false);
            let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
            let Some(target) = request_target(&mut stream, cancel, deadline) else {
                respond(&mut stream, 400, "Bad request");
                continue;
            };
            let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
            if path != "/auth/callback" {
                respond(&mut stream, 404, "Not found");
                continue;
            }
            let params = query_params(query);
            let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str()).unwrap_or("");
            if !super::callback_state_matches(&params, &p.state) {
                respond(&mut stream, 400, "State mismatch");
                continue;
            }
            if !get("error").is_empty() {
                let why = if get("error") == "access_denied" { "access denied" } else { "authorization failed" };
                respond(&mut stream, 200, &page("Sign-in failed", why));
                return Err(Error::new(if get("error") == "access_denied" { Recovery::Consent } else { Recovery::Auth }, why));
            }
            let code = get("code");
            if code.is_empty() {
                respond(&mut stream, 400, "Missing authorization code");
                continue;
            }
            // The code exchange runs to completion (its timeout is short), so that tokens issued
            // for an attempt that is then cancelled, unverified or not kept can be revoked.
            let exchanged = callback_client_id(&params, &p.client_id).map_err(Error::auth).and_then(|client_id| {
                let resp = http::send_timeout(&exchange_request(&client_id, code, &p.redirect_uri, &p.verifier), Duration::from_secs(30))
                    .map_err(Error::auth)?;
                let grant = parse_grant(&resp).map_err(|e| Error::token(&resp, e))?;
                Ok((client_id, grant))
            });
            let result = exchanged.and_then(|(client_id, grant)| {
                let kept = finish(p, &client_id, &grant, &mut |req| auth_send(req, cancel, deadline))
                    .and_then(|c| check_attempt(cancel, deadline).map(|()| c))
                    .and_then(|c| save_if_active(cancel, &c, &mut |c| save(c, p.expected.as_ref())).map(|()| c));
                if kept.is_err() && !grant.refresh_token.is_empty() {
                    let _ = revoke(&Revocation { client_id, refresh_token: grant.refresh_token.clone() });
                }
                kept
            });
            return match result {
                Ok(c) if !c.plan_allowed() => {
                    let why = "RustGIS wasn't allowed to use your ChatGPT plan. Sign in again and allow it to use the assistant.";
                    respond(&mut stream, 200, &page("Sign-in incomplete", why));
                    Ok(c)
                }
                Ok(c) => {
                    respond(&mut stream, 200, &page("Signed in", "RustGIS can now use your ChatGPT plan. You can close this tab."));
                    Ok(c)
                }
                Err(e) => {
                    respond(&mut stream, 200, &page("Sign-in failed", &e.to_string()));
                    Err(e)
                }
            };
        }
    }

    fn check_attempt(cancel: &Mutex<bool>, deadline: Instant) -> Result<(), Error> {
        if *cancel.lock().map_err(|_| "the sign-in was cancelled")? {
            return Err(Error::new(Recovery::Cancelled, "the sign-in was cancelled"));
        }
        if Instant::now() >= deadline {
            return Err(Error::new(
                Recovery::Timeout,
                "the sign-in timed out; restart sign-in and allow the browser to reach this computer's loopback address",
            ));
        }
        Ok(())
    }

    fn auth_send(req: &HttpRequest, cancel: &Mutex<bool>, deadline: Instant) -> Result<HttpResponse, Error> {
        let req = req.clone();
        let timeout = deadline.saturating_duration_since(Instant::now()).min(Duration::from_secs(30));
        cancellable(cancel, deadline, move || http::send_timeout(&req, timeout).map_err(Error::auth))
    }

    // The listener can exit immediately; a transport already in progress has a short timeout
    // and cannot persist anything after cancellation.
    fn cancellable<T: std::marker::Send + 'static>(
        cancel: &Mutex<bool>,
        deadline: Instant,
        work: impl FnOnce() -> Result<T, Error> + std::marker::Send + 'static,
    ) -> Result<T, Error> {
        check_attempt(cancel, deadline)?;
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        loop {
            check_attempt(cancel, deadline)?;
            match rx.recv_timeout(Duration::from_millis(25)) {
                Ok(result) => {
                    check_attempt(cancel, deadline)?;
                    return result;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err("the sign-in request stopped".into()),
            }
        }
    }

    /// Verifies who an exchanged grant signed in, as a new session.
    fn finish(p: &Pending, client_id: &str, grant: &super::Grant, send: Send) -> Result<Credentials, Error> {
        let account = verify_id_token(&grant.id_token, client_id, &p.nonce, send)?;
        if !p.subject.is_empty() && client_id == p.client_id && account.subject != p.subject {
            return Err(Error::new(Recovery::AccountChanged, "signing in: a different ChatGPT account came back than the one registered"));
        }
        let mut c = super::signed_in(client_id, &account, grant, now());
        c.session = random_token()?;
        Ok(c)
    }

    /// Keeps a verified sign-in, with plan permission recorded separately.
    fn save(c: &Credentials, expected: Option<&Credentials>) -> Result<(), Error> {
        let path = auth_path().ok_or("no settings folder")?;
        save_at(&path, c, expected)
    }

    fn save_at(path: &Path, c: &Credentials, expected: Option<&Credentials>) -> Result<(), Error> {
        let _guard = store(path)?;
        let current = read_accounts(path)?.selected().cloned();
        // Who, and whether signed in: a sign-out or another sign-in meanwhile is not undone.
        let identity = |c: &Credentials| (c.binding(), c.signed_in());
        if current.as_ref().map(identity) != expected.map(identity) {
            return Err(Error::new(Recovery::AccountChanged, "the ChatGPT sign-in changed; select the account again"));
        }
        // Keep valid identity even when plan permission was declined.
        write_credentials(path, c)
    }

    fn save_if_active(
        cancel: &Mutex<bool>,
        c: &Credentials,
        persist: &mut dyn FnMut(&Credentials) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let cancelled = cancel.lock().map_err(|_| "the sign-in was cancelled")?;
        if *cancelled {
            return Err(Error::new(Recovery::Cancelled, "the sign-in was cancelled"));
        }
        persist(c)
    }

    /// The request target (`/auth/callback?code=…`) of an HTTP request, read up to its headers' end.
    fn request_target(stream: &mut impl Read, cancel: &Mutex<bool>, deadline: Instant) -> Option<String> {
        let deadline = deadline.min(Instant::now() + Duration::from_secs(2));
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
            check_attempt(cancel, deadline).ok()?;
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(chunk.get(..n)?),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    continue;
                }
                Err(_) => return None,
            }
        }
        if !buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024 {
            return None;
        }
        let line = buf.split(|b| *b == b'\n').next()?;
        let line = std::str::from_utf8(line).ok()?.trim();
        let mut parts = line.split(' ');
        let (method, target) = (parts.next()?, parts.next()?);
        let version = parts.next()?;
        (method == "GET" && version == "HTTP/1.1" && parts.next().is_none()).then(|| target.to_string())
    }

    fn respond(stream: &mut TcpStream, status: u16, body: &str) {
        let reason = match status {
            200 => "OK",
            400 => "Bad Request",
            404 => "Not Found",
            _ => "Error",
        };
        let kind = if body.starts_with("<!DOCTYPE") { "text/html; charset=utf-8" } else { "text/plain; charset=utf-8" };
        let head = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(body.as_bytes())).and_then(|_| stream.flush());
    }

    fn escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }

    /// The page the browser shows at the end.
    fn page(title: &str, text: &str) -> String {
        format!(
            "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>RustGIS: {t}</title></head>\
<body style=\"font-family: system-ui, sans-serif; margin: 3em auto; max-width: 32em; text-align: center\">\
<h1>{t}</h1><p>{b}</p></body></html>",
            t = escape(title),
            b = escape(text)
        )
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn access(exp: i64) -> String {
            let payload = serde_json::json!({"client_id": "oaiapp_synthetic", "sub": "user-1", "exp": exp});
            format!("e30.{}.sig", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string()))
        }

        fn synthetic(expires_at: i64) -> Credentials {
            Credentials {
                client_id: "oaiapp_synthetic".into(),
                subject: "user-1".into(),
                email: "ann@example.com".into(),
                id_token: "synthetic-id".into(),
                access_token: access(expires_at),
                refresh_token: "synthetic-refresh".into(),
                expires_at,
                scopes: super::super::SCOPES.split(' ').map(str::to_string).collect(),
                plan_declined: false,
                saved_at: String::new(),
                session: "session-1".into(),
            }
        }

        fn grant(expires_at: i64, refresh: &str) -> HttpResponse {
            let body = serde_json::json!({"access_token": access(expires_at), "refresh_token": refresh, "expires_in": expires_at - now()});
            HttpResponse { status: 200, body: body.to_string().into_bytes(), request_id: None }
        }

        fn test_request() -> HttpRequest {
            HttpRequest {
                method: "POST",
                url: format!("{}/responses", super::super::API),
                headers: vec![("Authorization".into(), format!("Bearer {}", access(0)))],
                body: None,
            }
        }

        #[test]
        fn requests_for_another_registration_or_after_sign_out_are_refused() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("auth.json");
            for change in ["client", "subject", "signed-out"] {
                let mut c = synthetic(now() + 100_000);
                match change {
                    "client" => c.client_id = "other-registration".into(),
                    "subject" => c.subject = "other-user".into(),
                    _ => c = c.registration(),
                }
                write_credentials(&path, &c).unwrap();
                let e = send_at(&test_request(), &synthetic(0).binding(), &path, &mut |_| panic!("must refuse before any network I/O"))
                    .unwrap_err();
                assert!(e.to_string().contains(if change == "signed-out" { "not signed in" } else { "changed" }), "{change}: {e}");
            }
        }

        #[test]
        fn work_from_before_a_sign_out_is_refused_after_signing_in_again() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("auth.json");
            let old = synthetic(now() + 100_000);
            write_credentials(&path, &old).unwrap();
            // Another window signs out and back into the same registration.
            sign_out_at(&path).unwrap();
            let mut again = old.clone();
            again.session = "session-2".into();
            write_credentials(&path, &again).unwrap();
            let e = send_at(&test_request(), &old.binding(), &path, &mut |_| panic!("must refuse before any network I/O")).unwrap_err();
            assert!(e.to_string().contains("changed"), "{e}");
            // A sign-in begun before another process signs out is not kept, even when the account
            // was already signed out.
            sign_out_at(&path).unwrap();
            let started = read_credentials(&path).unwrap();
            sign_out_at(&path).unwrap();
            let mut late = old.clone();
            late.session = "session-3".into();
            assert!(save_at(&path, &late, Some(&started)).is_err());
            assert!(!read_credentials(&path).unwrap().signed_in());
            // Renewal and selection keep the session.
            write_credentials(&path, &again).unwrap();
            assert_eq!(select_at(&path, &again.client_id, &again.subject).unwrap().binding(), again.binding());
        }

        #[test]
        fn renewed_scope_is_checked_before_initial_send_and_retry() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("auth.json");
            for retry in [false, true] {
                write_credentials(&path, &synthetic(if retry { now() + 100_000 } else { 0 })).unwrap();
                let mut inference = 0;
                let e = send_at(&test_request(), &synthetic(0).binding(), &path, &mut |req| {
                    if req.url == super::super::TOKEN_URL {
                        let mut r = grant(now() + 100_000, "rotated");
                        let mut v: Value = serde_json::from_slice(&r.body).unwrap();
                        v["scope"] = serde_json::json!("openid email");
                        r.body = v.to_string().into_bytes();
                        return Ok(r);
                    }
                    inference += 1;
                    Ok(HttpResponse { status: 401, body: vec![], request_id: None })
                })
                .unwrap_err();
                assert!(e.to_string().contains("plan"));
                assert_eq!(inference, usize::from(retry));
                assert!(!read_credentials(&path).unwrap().plan_allowed());
            }
        }

        #[test]
        fn separate_store_handles_serialize_compare_and_write() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("auth.json");
            write_credentials(&path, &synthetic(0)).unwrap();
            let held = store(&path).unwrap();
            let other = std::fs::OpenOptions::new().read(true).write(true).open(path.with_extension("store-lock")).unwrap();
            assert!(other.try_lock().is_err());
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    let r = sign_out_at(&path);
                    tx.send(r).unwrap();
                });
                assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
                drop(held);
                assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap().is_ok());
            });
            assert!(!read_credentials(&path).unwrap().signed_in());
            assert!(other.try_lock().is_ok());
        }

        #[test]
        fn independent_process_changes_survive_inflight_renewal_and_login() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("auth.json");
            for signout in [false, true] {
                let old = synthetic(0);
                write_credentials(&path, &old).unwrap();
                let result = send_at(&test_request(), &synthetic(0).binding(), &path, &mut |_| {
                    std::thread::scope(|scope| {
                        scope
                            .spawn(|| {
                                if signout {
                                    sign_out_at(&path).unwrap();
                                } else {
                                    let mut new = old.clone();
                                    new.subject = "new-account".into();
                                    save_at(&path, &new, Some(&old)).unwrap();
                                }
                            })
                            .join()
                            .unwrap();
                    });
                    Ok(grant(now() + 100_000, "late-refresh"))
                });
                assert!(result.is_err());
                assert!(save_at(&path, &old, Some(&old)).is_err(), "late login must not undo the change");
                let current = read_credentials(&path).unwrap();
                assert_ne!(current.refresh_token, "late-refresh");
                assert_eq!(current.signed_in(), !signout);
            }
        }

        #[test]
        fn accounts_migrate_select_and_keep_distinct_registrations() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("auth.json");
            let a = synthetic(now() + 100_000);
            std::fs::write(&path, a.to_json().to_string()).unwrap();
            assert_eq!(read_credentials(&path).unwrap(), a);
            let mut b = a.clone();
            b.client_id = "oaiapp_workspace".into();
            assert_ne!(a.label(), b.label());
            save_at(&path, &b, Some(&a)).unwrap();
            let selected = select_at(&path, &a.client_id, &a.subject).unwrap();
            assert_eq!(selected, a, "selecting changes nothing but the selection");
            assert_eq!(selected.client_id, a.client_id);
            sign_out_at(&path).unwrap();
            let accounts = read_accounts(&path).unwrap();
            assert_eq!(accounts.entries.len(), 2);
            assert!(!accounts.selected().unwrap().signed_in());
            let selected = select_at(&path, &b.client_id, &b.subject).unwrap();
            assert!(selected.signed_in());
            assert_eq!(selected.refresh_token, b.refresh_token);
            let json: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(json["version"], 2);
        }

        #[test]
        fn callback_reads_and_auth_requests_obey_cancel_and_deadline() {
            let cancel = Arc::new(Mutex::new(false));
            struct Trickle;
            impl Read for Trickle {
                fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                    std::thread::sleep(Duration::from_millis(10));
                    if let Some(b) = buf.first_mut() {
                        *b = b'x';
                    }
                    Ok(1)
                }
            }
            let mut socket = Trickle;
            let start = Instant::now();
            assert!(request_target(&mut socket, &cancel, start + Duration::from_millis(60)).is_none());
            assert!(start.elapsed() < Duration::from_secs(1));
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    std::thread::sleep(Duration::from_millis(30));
                    *cancel.lock().unwrap() = true;
                });
                let e = cancellable(&cancel, Instant::now() + Duration::from_secs(10), || {
                    std::thread::sleep(Duration::from_millis(150));
                    Ok(())
                })
                .unwrap_err();
                assert!(e.to_string().contains("cancelled"));
            });
            assert!(request_target(&mut socket, &cancel, Instant::now() + Duration::from_secs(10)).is_none());
            let e = cancellable(&Mutex::new(false), Instant::now() + Duration::from_millis(20), || {
                std::thread::sleep(Duration::from_millis(100));
                Ok(())
            })
            .unwrap_err();
            assert!(e.to_string().contains("timed out"));
        }

        #[test]
        fn simultaneous_401_responses_rotate_only_once() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("test-auth.json");
            let far = now() + 100_000;
            write_credentials(&path, &synthetic(far)).unwrap();
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let rotations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            std::thread::scope(|scope| {
                let workers: Vec<_> = (0..2)
                    .map(|_| {
                        let (barrier, rotations, path) = (barrier.clone(), rotations.clone(), path.clone());
                        scope.spawn(move || {
                            let mut inference = 0;
                            send_at(&test_request(), &synthetic(0).binding(), &path, &mut |req| {
                                if req.url == super::super::TOKEN_URL {
                                    rotations.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                    return Ok(grant(far + 1, "synthetic-rotated"));
                                }
                                inference += 1;
                                if inference == 1 {
                                    barrier.wait();
                                    Ok(HttpResponse { status: 401, body: vec![], request_id: None })
                                } else {
                                    let bearer = format!("Bearer {}", access(far + 1));
                                    assert!(req.headers.iter().any(|(k, v)| k == "Authorization" && *v == bearer));
                                    Ok(HttpResponse { status: 200, body: vec![], request_id: None })
                                }
                            })
                            .unwrap()
                        })
                    })
                    .collect();
                for worker in workers {
                    assert_eq!(worker.join().unwrap().status, 200);
                }
            });
            assert_eq!(rotations.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert_eq!(read_credentials(&path).unwrap().refresh_token, "synthetic-rotated");
        }

        #[test]
        fn late_401_does_not_restore_a_signed_out_or_different_account() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("test-auth.json");
            for switch_account in [false, true] {
                write_credentials(&path, &synthetic(now() + 100_000)).unwrap();
                let mut requests = 0;
                let result = send_at(&test_request(), &synthetic(0).binding(), &path, &mut |_| {
                    requests += 1;
                    let _guard = store(&path).unwrap();
                    if switch_account {
                        let mut other = synthetic(now() + 100_001);
                        other.subject = "user-2".into();
                        write_credentials(&path, &other).unwrap();
                    } else {
                        write_credentials(&path, &synthetic(0).registration()).unwrap();
                    }
                    Ok(HttpResponse { status: 401, body: vec![], request_id: None })
                });
                assert!(result.is_err());
                assert_eq!(requests, 1, "a late response must not renew or resend");
                assert_eq!(read_credentials(&path).unwrap().signed_in(), switch_account);
            }
        }

        #[test]
        fn sign_out_during_refresh_does_not_wait_for_network_or_restore_tokens() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("test-auth.json");
            write_credentials(&path, &synthetic(0)).unwrap();
            let result = send_at(&test_request(), &synthetic(0).binding(), &path, &mut |req| {
                assert_eq!(req.url, super::super::TOKEN_URL);
                // An independent handle can sign out while renewal waits for the network.
                sign_out_at(&path).unwrap();
                Ok(grant(now() + 100_000, "synthetic-rotated"))
            });
            assert!(result.is_err());
            assert!(!read_credentials(&path).unwrap().signed_in());
        }

        #[test]
        fn an_unusable_refresh_token_is_cleared_and_the_registration_kept() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("test-auth.json");
            write_credentials(&path, &synthetic(0)).unwrap();
            let e = send_at(&test_request(), &synthetic(0).binding(), &path, &mut |_| {
                Ok(HttpResponse { status: 400, body: br#"{"error":"refresh_token_reused"}"#.to_vec(), request_id: None })
            })
            .unwrap_err();
            assert!(e.to_string().contains("sign in again"), "{e}");
            let left = read_credentials(&path).unwrap();
            assert!(!left.signed_in() && left.refresh_token.is_empty());
            assert_eq!(left.client_id, "oaiapp_synthetic");
            // A passing failure keeps the tokens.
            write_credentials(&path, &synthetic(0)).unwrap();
            assert!(
                send_at(&test_request(), &synthetic(0).binding(), &path, &mut |_| Ok(HttpResponse {
                    status: 503,
                    body: vec![],
                    request_id: None
                }))
                .is_err()
            );
            assert!(read_credentials(&path).unwrap().signed_in());
        }

        #[test]
        fn without_the_plan_nothing_is_sent() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("test-auth.json");
            let mut c = synthetic(now() + 100_000);
            c.scopes.retain(|s| s != super::super::PLAN_SCOPE);
            write_credentials(&path, &c).unwrap();
            let e = send_at(&test_request(), &synthetic(0).binding(), &path, &mut |_| panic!("nothing may be sent")).unwrap_err();
            assert!(e.to_string().contains("plan"), "{e}");
        }

        #[test]
        fn sign_out_keeps_the_registration_and_gives_the_session_to_revoke() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("test-auth.json");
            write_credentials(&path, &synthetic(now() + 100_000)).unwrap();
            let r = sign_out_at(&path).unwrap().unwrap();
            assert_eq!((r.client_id.as_str(), r.refresh_token.as_str()), ("oaiapp_synthetic", "synthetic-refresh"));
            assert!(!format!("{r:?}").contains("synthetic-refresh"));
            let left = read_credentials(&path).unwrap();
            assert!(!left.signed_in() && left.id_token.is_empty() && left.refresh_token.is_empty());
            assert_eq!((left.client_id.as_str(), left.subject.as_str()), ("oaiapp_synthetic", "user-1"));
            // Again: nothing more to revoke. A missing file is no error.
            assert!(sign_out_at(&path).unwrap().is_none());
            assert!(sign_out_at(&dir.path().join("missing.json")).unwrap().is_none());
        }

        #[test]
        fn revocation_retries_server_errors_only() {
            let r = Revocation { client_id: "oaiapp_synthetic".into(), refresh_token: "synthetic-refresh".into() };
            let config = serde_json::json!({"revocation_endpoint": "https://auth.openai.com/api/accounts/oauth/revoke"}).to_string();
            for (statuses, ok, attempts) in
                [(vec![200], true, 1), (vec![503, 502, 200], true, 3), (vec![503, 503, 503], false, 3), (vec![400], false, 1)]
            {
                let mut replies = statuses.into_iter();
                let (mut sent, mut waits) = (0, 0);
                let result = revoke_with(
                    &r,
                    &mut |req| {
                        if req.url == super::super::DISCOVERY_URL {
                            return Ok(HttpResponse { status: 200, body: config.clone().into_bytes(), request_id: None });
                        }
                        assert_eq!(req.url, "https://auth.openai.com/api/accounts/oauth/revoke");
                        assert!(
                            req.body
                                .as_deref()
                                .is_some_and(|b| b.contains("token=synthetic-refresh") && b.contains("client_id=oaiapp_synthetic"))
                        );
                        sent += 1;
                        Ok(HttpResponse { status: replies.next().unwrap_or(500), body: vec![], request_id: None })
                    },
                    &mut |_| waits += 1,
                );
                assert_eq!((result.is_ok(), sent, waits), (ok, attempts, attempts - 1));
            }
            // A failed discovery is retried too.
            let mut discoveries = 0;
            let result = revoke_with(
                &r,
                &mut |req| {
                    if req.url == super::super::DISCOVERY_URL {
                        discoveries += 1;
                        return if discoveries == 1 {
                            Err("offline".into())
                        } else {
                            Ok(HttpResponse { status: 200, body: config.clone().into_bytes(), request_id: None })
                        };
                    }
                    Ok(HttpResponse { status: 200, body: vec![], request_id: None })
                },
                &mut |_| {},
            );
            assert!(result.is_ok());
            assert_eq!(discoveries, 2);
        }

        #[test]
        fn cancellation_prevents_persistence() {
            let cancelled = Mutex::new(true);
            let mut writes = 0;
            let result = save_if_active(&cancelled, &Credentials::default(), &mut |_| {
                writes += 1;
                Ok(())
            });
            assert!(result.is_err());
            assert_eq!(writes, 0);
            let result = save_if_active(&Mutex::new(false), &Credentials::default(), &mut |_| Err("disk full".into()));
            assert_eq!(result, Err("disk full".into()));
        }

        #[test]
        fn credentials_file_replaces_atomically_and_cleans_failed_writes() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("test-auth.json");
            let first = synthetic(1);
            let next = synthetic(2);
            write_credentials(&path, &first).unwrap();
            write_credentials(&path, &next).unwrap();
            assert_eq!(read_credentials(&path).unwrap(), next);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            }
            let destination_dir = dir.path().join("directory");
            std::fs::create_dir(&destination_dir).unwrap();
            assert!(write_credentials(&destination_dir, &next).is_err());
            assert_eq!(read_credentials(&path).unwrap(), next);
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2, "failed writes leave no token-bearing tempfiles");
            // A byte-order mark (Windows editors) is no reason to lose the sign-in.
            std::fs::write(&path, format!("\u{feff}{}", next.to_json())).unwrap();
            assert_eq!(read_credentials(&path).unwrap(), next);
        }
    }
}
