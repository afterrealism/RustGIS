//! Errors with recovery actions, kept separate from diagnostic text.

use serde_json::Value;

/// The action the user can take after a failed request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    /// A provider or transport diagnostic.
    Other,
    /// Restart authorization after a terminal credential error.
    SignIn,
    /// Select the intended saved registration again.
    AccountChanged,
    /// Enable plan use explicitly in settings.
    Consent,
    /// Restart authorization and check the loopback callback.
    Timeout,
    /// The user stopped authorization.
    Cancelled,
    /// Check local storage, browser, or identity verification.
    Auth,
    /// Inspect client, grant, policy, region, or route configuration.
    Configuration,
    /// Inspect the unsupported request parameter.
    Unsupported,
    /// The account or workspace is not eligible.
    Ineligible,
    /// Pause requests and open weekly usage settings.
    Usage,
    /// Preserve credentials and retry later.
    Later,
}

/// A failure and its diagnostic context. Diagnostics are not translated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// The recovery action, independent of the server's wording.
    pub recovery: Recovery,
    /// HTTP status, including 200 for a failed stream.
    pub status: Option<u16>,
    /// The exact server code, when supplied.
    pub code: Option<String>,
    /// The unsupported parameter, when supplied.
    pub param: Option<String>,
    /// The server's request id.
    pub request_id: Option<String>,
    /// Whether the server returned an error object, detail, or another shape.
    pub shape: &'static str,
    /// A bounded diagnostic. Never put OAuth response bodies here.
    pub detail: String,
}

impl Error {
    /// A local authentication failure; OAuth diagnostics are already credential-free.
    pub fn auth(detail: String) -> Self {
        Self::new(Recovery::Auth, detail)
    }

    /// A local failure with an explicit recovery action.
    pub fn new(recovery: Recovery, detail: impl Into<String>) -> Self {
        Self { recovery, ..Self::from(detail.into()) }
    }

    /// Adds authentication context to a storage or transport diagnostic.
    pub fn auth_context(mut self) -> Self {
        if self.recovery == Recovery::Other {
            self.recovery = Recovery::Auth;
        }
        self
    }

    /// A token error, retaining protocol fields without echoing credentials.
    pub fn token(resp: &crate::provider::HttpResponse, grant: crate::chatgpt::GrantError) -> Self {
        let v: Value = serde_json::from_slice(&resp.body).unwrap_or(Value::Null);
        let code = v.get("error").and_then(|v| v.as_str().or_else(|| v.get("code").and_then(Value::as_str)));
        let recovery = match &grant {
            crate::chatgpt::GrantError::Unusable => Recovery::SignIn,
            crate::chatgpt::GrantError::Other(code)
                if matches!(
                    code.as_str(),
                    "invalid_client" | "invalid_scope" | "invalid_request" | "unauthorized_client" | "unsupported_grant_type"
                ) =>
            {
                Recovery::Configuration
            }
            _ => Recovery::Auth,
        };
        let code = code
            .filter(|code| {
                matches!(
                    *code,
                    "invalid_grant"
                        | "invalid_refresh_token"
                        | "token_expired"
                        | "refresh_token_expired"
                        | "refresh_token_invalidated"
                        | "refresh_token_reused"
                        | "invalid_client"
                        | "invalid_scope"
                        | "invalid_request"
                        | "unauthorized_client"
                        | "unsupported_grant_type"
                )
            })
            .map(str::to_string);
        Self {
            status: Some(resp.status),
            request_id: resp.request_id.clone(),
            code,
            shape: "oauth",
            ..Self::new(recovery, grant.message("ChatGPT"))
        }
    }

    /// An API error, including bare admission errors and streamed failures.
    pub fn response(v: &Value, resp: &crate::provider::HttpResponse) -> Self {
        let e = v.get("error").or_else(|| v.pointer("/response/error"));
        let field = |k: &str| e.and_then(|e| e.get(k)).and_then(Value::as_str).map(str::to_string);
        let code = field("code");
        let recovery = match code.as_deref() {
            Some("subscription_sharing_usage_limit_exceeded") => Recovery::Usage,
            Some("subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable") => Recovery::Later,
            Some("subscription_sharing_user_not_eligible") => Recovery::Ineligible,
            Some("subscription_sharing_unsupported_capability") => Recovery::Unsupported,
            Some(
                "chatpass_v2_scope_not_authorized"
                | "chatpass_v2_invalid_authorization_context"
                | "subscription_sharing_route_not_supported"
                | "subscription_sharing_invalid_user",
            ) => Recovery::Configuration,
            _ => match resp.status {
                401 | 403 => Recovery::Configuration,
                503 => Recovery::Later,
                _ => Recovery::Other,
            },
        };
        let detail = field("message")
            .or_else(|| v.get("detail").and_then(Value::as_str).map(str::to_string))
            .or_else(|| v.get("message").and_then(Value::as_str).map(str::to_string))
            .or_else(|| v.is_null().then(|| String::from_utf8_lossy(&resp.body).chars().take(500).collect()))
            .unwrap_or_else(|| "the model's reply failed".into());
        Self {
            recovery,
            status: Some(resp.status),
            code,
            param: field("param"),
            request_id: resp.request_id.clone(),
            shape: if e.is_some() {
                "error"
            } else if v.get("detail").is_some() {
                "detail"
            } else {
                "other"
            },
            detail: detail.chars().take(500).collect(),
        }
    }

    /// The English recovery template; the UI translates it before display.
    pub fn message(&self) -> &str {
        match self.recovery {
            Recovery::Other => &self.detail,
            Recovery::SignIn => "The sign-in expired or was revoked. Sign in again.",
            Recovery::AccountChanged => "The saved sign-in changed. Select your account again.",
            Recovery::Consent => {
                "Allow ChatGPT plan use when signing in. If your workspace doesn't permit it, ask its admin or choose an API key provider."
            }
            Recovery::Timeout => "Sign-in timed out. Start again and allow the browser to reach this computer's loopback address.",
            Recovery::Cancelled => "Sign-in cancelled.",
            Recovery::Auth => "ChatGPT sign-in failed. Check the connection and local settings, then try again.",
            Recovery::Configuration => "Check the integration's client, permissions, workspace policy and route configuration.",
            Recovery::Unsupported => "This request uses an unsupported capability. Check the parameter and change the request.",
            Recovery::Ineligible => "ChatGPT plan use is unavailable for this account or workspace.",
            Recovery::Usage => "A ChatGPT plan or app usage limit was reached. Review Weekly usage before trying again.",
            Recovery::Later => "ChatGPT usage is temporarily unavailable. Keep your sign-in and try again later.",
        }
    }
}

impl From<String> for Error {
    fn from(detail: String) -> Self {
        Self { recovery: Recovery::Other, status: None, code: None, param: None, request_id: None, shape: "", detail }
    }
}

impl From<&str> for Error {
    fn from(detail: &str) -> Self {
        detail.to_string().into()
    }
}

impl From<Error> for String {
    fn from(e: Error) -> Self {
        e.to_string()
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(status) = self.status {
            write!(f, "HTTP {status}: ")?;
        }
        write!(f, "{}", self.message())?;
        if self.recovery != Recovery::Other && !self.detail.is_empty() {
            write!(f, " ({})", self.detail)?;
        }
        for v in [&self.code, &self.param, &self.request_id].into_iter().flatten() {
            write!(f, " [{v}]")?;
        }
        if self.recovery == Recovery::Usage {
            write!(f, " {}", crate::chatgpt::USAGE_URL)?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}
