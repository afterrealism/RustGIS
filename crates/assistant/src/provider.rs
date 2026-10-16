//! Model providers: building the HTTP request for a conversation and reading the reply, for the
//! four wire formats the assistant speaks (OpenAI Chat Completions, which OpenRouter, Ollama and
//! most local servers also speak; OpenAI Responses, which the ChatGPT sign-in uses; Anthropic
//! Messages; Gemini `generateContent`).
//!
//! Nothing here does I/O: a request is plain data ([`HttpRequest`]) that the desktop sends on a
//! thread and the browser sends with its `fetch`.

use crate::Error;
use serde_json::{Map, Value, json};

/// A wire format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    /// `POST {base}/chat/completions`.
    OpenAi,
    /// `POST {base}/responses` (streamed): the OpenAI Responses API, as a ChatGPT plan's sign-in uses it.
    Responses,
    /// `POST {base}/v1/messages`.
    Anthropic,
    /// `POST {base}/models/{model}:generateContent`.
    Gemini,
}

/// A provider the settings offer, with its defaults.
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub api: Api,
    pub base_url: &'static str,
    pub model: &'static str,
    /// Environment variables an API key is read from (desktop and command line), in order.
    pub key_env: &'static [&'static str],
    /// An environment variable that overrides the base URL.
    pub url_env: &'static str,
    /// Whether it works without a key.
    pub keyless: bool,
    /// Where to get a key.
    pub key_url: &'static str,
    /// Signs in with an account (see [`crate::chatgpt`]) instead of taking a key: the key is the
    /// account's access token, kept by the sign-in, never typed or saved with the preferences.
    pub login: bool,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "anthropic",
        label: "Anthropic",
        api: Api::Anthropic,
        base_url: "https://api.anthropic.com",
        model: "claude-opus-5-5",
        key_env: &["ANTHROPIC_API_KEY"],
        url_env: "",
        keyless: false,
        key_url: "https://console.anthropic.com/settings/keys",
        login: false,
    },
    Preset {
        id: "openai",
        label: "OpenAI",
        api: Api::OpenAi,
        base_url: "https://api.openai.com/v1",
        model: "gpt-5.5",
        key_env: &["OPENAI_API_KEY"],
        url_env: "",
        keyless: false,
        key_url: "https://platform.openai.com/api-keys",
        login: false,
    },
    Preset {
        id: "chatgpt",
        label: "ChatGPT (sign in)",
        api: Api::Responses,
        base_url: crate::chatgpt::API,
        // OpenAI's example model; the automatically loaded account catalog says
        // which it may use.
        model: "gpt-6.1-sol",
        key_env: &[],
        url_env: "",
        keyless: false,
        key_url: crate::chatgpt::USAGE_URL,
        login: true,
    },
    Preset {
        id: "gemini",
        label: "Google Gemini",
        api: Api::Gemini,
        base_url: "https://generativelanguage.googleapis.com/v1beta",
        model: "gemini-3.8-flash",
        key_env: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        url_env: "",
        keyless: false,
        key_url: "https://aistudio.google.com/apikey",
        login: false,
    },
    Preset {
        id: "openrouter",
        label: "OpenRouter",
        api: Api::OpenAi,
        base_url: "https://openrouter.ai/api/v1",
        model: "openrouter/auto",
        key_env: &["OPENROUTER_API_KEY"],
        url_env: "",
        keyless: false,
        key_url: "https://openrouter.ai/keys",
        login: false,
    },
    Preset {
        id: "ollama",
        label: "Ollama (local)",
        api: Api::OpenAi,
        base_url: "http://localhost:11434/v1",
        model: "qwen3",
        key_env: &[],
        url_env: "OLLAMA_BASE_URL",
        keyless: true,
        key_url: "https://ollama.com/download",
        login: false,
    },
    Preset {
        id: "custom",
        label: "OpenAI-compatible",
        api: Api::OpenAi,
        base_url: "http://localhost:1234/v1",
        model: "",
        key_env: &["OPENAI_COMPATIBLE_API_KEY"],
        url_env: "OPENAI_COMPATIBLE_BASE_URL",
        keyless: true,
        key_url: "",
        login: false,
    },
];

/// The preset with this id.
pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// Which model to call, where and with what key.
#[derive(Clone, PartialEq)]
pub struct Config {
    /// A [`Preset`] id.
    pub provider: String,
    pub base_url: String,
    pub model: String,
    /// The API key, or the access token of a signed-in account.
    pub key: String,
    /// The signed-in ChatGPT account (its id token's subject); empty for other providers.
    pub account: String,
    /// The selected registration, including when signed out.
    pub client_id: String,
    /// The sign-in session of a signed-in account.
    pub session: String,
    /// Whether the catalog offers reasoning for the selected model.
    pub reasoning: Option<bool>,
    /// Requested tool output token estimate (default 10,000); the 24,000-token hard limit still applies.
    pub tool_output_token_limit: usize,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config").field("provider", &self.provider).field("model", &self.model).finish_non_exhaustive()
    }
}

impl Config {
    /// The preset's defaults, with a key and base URL from the environment where set (for a
    /// sign-in provider, the account signed in on this computer).
    pub fn from_env(provider: &str) -> Config {
        let p = preset(provider).unwrap_or(&PRESETS[0]);
        let env = |name: &str| std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let key = p.key_env.iter().find_map(|k| env(k)).unwrap_or_default();
        let mut base_url = if p.url_env.is_empty() { None } else { env(p.url_env) }.unwrap_or_else(|| p.base_url.to_string());
        // `OLLAMA_BASE_URL` names the server, not its OpenAI-compatible API.
        if p.id == "ollama" && !base_url.trim_end_matches('/').ends_with("/v1") {
            base_url = format!("{}/v1", base_url.trim_end_matches('/'));
        }
        let cfg = Config {
            provider: p.id.to_string(),
            base_url,
            model: p.model.to_string(),
            key,
            account: String::new(),
            client_id: String::new(),
            session: String::new(),
            reasoning: None,
            tool_output_token_limit: env("RUSTGIS_ASSISTANT_TOOL_OUTPUT_TOKEN_LIMIT")
                .and_then(|s| s.parse().ok())
                .filter(|n| *n > 0)
                .unwrap_or(crate::tools::DEFAULT_OUTPUT_TOKENS),
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut cfg = cfg;
            if p.login
                && let Some(t) = crate::chatgpt::load_any()
            {
                cfg.sign_in(&t);
            }
            cfg
        }
        #[cfg(target_arch = "wasm32")]
        {
            cfg
        }
    }

    /// Uses a signed-in account's access token.
    pub fn sign_in(&mut self, c: &crate::chatgpt::Credentials) {
        self.key = if c.plan_allowed() { c.access_token.clone() } else { String::new() };
        self.client_id = c.client_id.clone();
        self.account = c.subject.clone();
        self.session = c.session.clone();
    }

    /// The ChatGPT registration this configuration's requests are for.
    pub fn binding(&self) -> crate::chatgpt::Binding {
        crate::chatgpt::Binding { client_id: self.client_id.clone(), subject: self.account.clone(), session: self.session.clone() }
    }

    /// The provider wire format.
    pub fn api(&self) -> Api {
        preset(&self.provider).map_or(Api::OpenAi, |p| p.api)
    }

    /// Whether the provider signs in with an account rather than a key.
    pub fn login(&self) -> bool {
        preset(&self.provider).is_some_and(|p| p.login)
    }

    /// Why this configuration can't be used yet, if it can't.
    pub fn missing(&self) -> Option<&'static str> {
        let keyless = preset(&self.provider).is_some_and(|p| p.keyless);
        if self.model.trim().is_empty() {
            Some("Choose a model.")
        } else if self.base_url.trim().is_empty() {
            Some("Enter the provider's address.")
        } else if self.key.trim().is_empty() && self.login() {
            Some("Sign in with your ChatGPT account.")
        } else if self.key.trim().is_empty() && !keyless {
            Some("Enter an API key.")
        } else {
            None
        }
    }

    fn base(&self) -> &str {
        self.base_url.trim().trim_end_matches('/')
    }
}

/// An HTTP request to make.
#[derive(Clone, PartialEq)]
pub struct HttpRequest {
    /// The HTTP method.
    pub method: &'static str,
    /// The destination; may contain secrets.
    pub url: String,
    /// Request headers; may contain credentials.
    pub headers: Vec<(String, String)>,
    /// The request payload; never logged by Debug.
    pub body: Option<String>,
}

/// What came back.
#[derive(Clone, PartialEq)]
pub struct HttpResponse {
    /// The HTTP status.
    pub status: u16,
    /// The response payload; may contain credentials.
    pub body: Vec<u8>,
    /// The server request id, when supplied.
    pub request_id: Option<String>,
}

impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRequest").field("method", &self.method).finish_non_exhaustive()
    }
}

impl std::fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpResponse").field("status", &self.status).field("request_id", &self.request_id).finish_non_exhaustive()
    }
}

/// The largest reply read.
pub const MAX_RESPONSE: usize = 8 << 20;

/// The largest streamed reply read (an event stream repeats the reply in its deltas and, for a
/// ChatGPT plan, carries the encrypted reasoning too).
pub const MAX_STREAM: usize = 64 << 20;

/// The largest reply to read for `req`: [`MAX_STREAM`] when it asks for an event stream.
pub fn reply_limit(req: &HttpRequest) -> usize {
    let stream = req.headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("accept") && v.contains("text/event-stream"));
    if stream { MAX_STREAM } else { MAX_RESPONSE }
}

/// Whether the assistant may send a request (with an API key) to `url`: HTTPS anywhere, plain
/// HTTP only to this computer or the local network (Ollama, LM Studio).
pub fn allowed_url(url: &str) -> bool {
    let url = url.trim();
    let (https, rest) = match (url.strip_prefix("https://"), url.strip_prefix("http://")) {
        (Some(r), _) => (true, r),
        (None, Some(r)) => (false, r),
        _ => return false,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // No credentials in the address (they would be echoed in errors), and
    // `http://localhost@evil.example` is not local.
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    if https {
        return true;
    }
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    }
    .to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") || host == "::1" {
        return true;
    }
    // `010.0.0.1` is octal to the system's resolver (8.0.0.1), not 10.0.0.1.
    if host.split('.').any(|p| p.len() > 1 && p.starts_with('0')) {
        return false;
    }
    let octets: Vec<u8> = host.split('.').filter_map(|p| p.parse().ok()).collect();
    if octets.len() != 4 || host.split('.').count() != 4 {
        return false;
    }
    match (octets.first(), octets.get(1)) {
        (Some(127), _) | (Some(10), _) => true,
        (Some(192), Some(168)) => true,
        (Some(172), Some(b)) => (16..=31).contains(b),
        _ => false,
    }
}

/// A tool the model may call.
#[derive(Clone, Debug)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    /// A JSON Schema object (`{"type": "object", "properties": …}`).
    pub schema: Value,
}

/// A call the model made.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

/// What a call gave back.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub id: String,
    pub name: String,
    pub content: String,
    pub error: bool,
}

/// One turn of the conversation, in no provider's format.
#[derive(Clone, Debug)]
pub enum Turn {
    User(String),
    Assistant {
        text: String,
        calls: Vec<ToolCall>,
        /// The reply as the provider sent it, sent back unchanged to the same provider: it may carry
        /// what the model needs to see again (Claude's thinking blocks, Gemini's thought signatures).
        native: Option<(Api, Value)>,
    },
    /// The results of the calls in the assistant turn before.
    Results(Vec<ToolResult>),
}

/// A reply read from the provider.
#[derive(Clone, Debug)]
pub struct Reply {
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub native: Value,
    /// Why the reply stopped early (length limit, a refusal), if it did.
    pub cut: Option<String>,
    /// A failed terminal response accompanying partial text.
    pub failure: Option<Error>,
}

/// Replies may be long (tables, explanations) but are never documents.
const MAX_TOKENS: u32 = 16_000;

/// The request asking the model for its next turn.
pub fn chat_request(cfg: &Config, system: &str, turns: &[Turn], tools: &[ToolDef]) -> HttpRequest {
    match cfg.api() {
        Api::OpenAi => openai_request(cfg, system, turns, tools),
        Api::Responses => responses_request(cfg, system, turns, tools),
        Api::Anthropic => anthropic_request(cfg, system, turns, tools),
        Api::Gemini => gemini_request(cfg, system, turns, tools),
    }
}

/// Reads a chat reply (any status).
pub fn parse_reply(api: Api, resp: &HttpResponse) -> Result<Reply, Error> {
    let parse = match api {
        Api::Responses => return responses_reply(resp),
        Api::OpenAi => openai_reply,
        Api::Anthropic => anthropic_reply,
        Api::Gemini => gemini_reply,
    };
    let v: Value = serde_json::from_slice(&resp.body).map_err(|_| http_error(resp))?;
    if !(200..300).contains(&resp.status) {
        return Err(error_message(&v).unwrap_or_else(|| http_error(resp)).into());
    }
    parse(&v).map_err(Error::from)
}

fn http_error(resp: &HttpResponse) -> String {
    let text: String = String::from_utf8_lossy(&resp.body).chars().take(300).collect();
    if (200..300).contains(&resp.status) {
        format!("the provider sent something that isn't JSON: {text}")
    } else {
        format!("HTTP {}: {}", resp.status, text.trim())
    }
}

/// The message of an error body (the providers nest it under `error`; the ChatGPT plan's
/// admission check sometimes says `detail`).
fn error_message(v: &Value) -> Option<String> {
    let msg = match v.get("error") {
        Some(e) => e.get("message").and_then(Value::as_str).or_else(|| e.as_str())?,
        None => v.get("detail").and_then(|d| d.as_str().or_else(|| d.get("message").and_then(Value::as_str)))?,
    };
    Some(msg.chars().take(500).collect())
}

fn json_body(v: &Value) -> Option<String> {
    Some(v.to_string())
}

fn header(k: &str, v: &str) -> (String, String) {
    (k.to_string(), v.to_string())
}

// ---------------------------------------------------------------------------------------------
// OpenAI Chat Completions

fn openai_request(cfg: &Config, system: &str, turns: &[Turn], tools: &[ToolDef]) -> HttpRequest {
    let mut messages = vec![json!({"role": "system", "content": system})];
    for t in turns {
        match t {
            Turn::User(text) => messages.push(json!({"role": "user", "content": text})),
            Turn::Assistant { native: Some((Api::OpenAi, m)), .. } => messages.push(m.clone()),
            Turn::Assistant { text, calls, .. } => {
                let mut m = json!({"role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) }});
                if !calls.is_empty() {
                    m["tool_calls"] = calls
                        .iter()
                        .map(|c| json!({"id": c.id, "type": "function", "function": {"name": c.name, "arguments": c.args.to_string()}}))
                        .collect();
                }
                messages.push(m);
            }
            Turn::Results(rs) => {
                for r in rs {
                    messages.push(json!({"role": "tool", "tool_call_id": r.id, "content": r.content}));
                }
            }
        }
    }
    let tools: Vec<Value> = tools
        .iter()
        .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.schema}}))
        .collect();
    let body = json!({"model": cfg.model.trim(), "messages": messages, "tools": tools});
    let mut headers = vec![header("Content-Type", "application/json")];
    if !cfg.key.trim().is_empty() {
        headers.push(header("Authorization", &format!("Bearer {}", cfg.key.trim())));
    }
    HttpRequest { method: "POST", url: format!("{}/chat/completions", cfg.base()), headers, body: json_body(&body) }
}

fn openai_reply(v: &Value) -> Result<Reply, String> {
    let choice = v.get("choices").and_then(|c| c.get(0)).ok_or("the reply has no choices")?;
    let msg = choice.get("message").ok_or("the reply has no message")?;
    let text = msg.get("content").and_then(Value::as_str).unwrap_or("").to_string();
    let calls = msg
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(i, c)| {
                    let f = c.get("function");
                    let args = f.and_then(|f| f.get("arguments"));
                    ToolCall {
                        id: c.get("id").and_then(Value::as_str).map_or_else(|| format!("call_{i}"), str::to_string),
                        name: f.and_then(|f| f.get("name")).and_then(Value::as_str).unwrap_or("").to_string(),
                        // Arguments come as JSON text; some servers send an object.
                        args: match args {
                            Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| json!({"_unparsed": s})),
                            Some(other) => other.clone(),
                            None => json!({}),
                        },
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let cut = match choice.get("finish_reason").and_then(Value::as_str) {
        Some("length") => Some("the reply reached its length limit".to_string()),
        Some("content_filter") => Some("the provider's content filter stopped the reply".to_string()),
        _ => None,
    };
    // Replayed as sent, minus fields some servers refuse in requests.
    let mut native = msg.clone();
    if let Some(o) = native.as_object_mut() {
        o.retain(|k, _| matches!(k.as_str(), "role" | "content" | "tool_calls" | "reasoning_content"));
    }
    Ok(Reply { text, calls, native, cut, failure: None })
}

// ---------------------------------------------------------------------------------------------
// OpenAI Responses (a ChatGPT plan's sign-in)

/// The namespace the assistant's tools are grouped in (a ChatGPT plan takes namespaced tools).
pub const TOOL_NAMESPACE: &str = "rustgis";

fn responses_headers(cfg: &Config) -> Vec<(String, String)> {
    vec![header("Authorization", &format!("Bearer {}", cfg.key.trim()))]
}

fn responses_request(cfg: &Config, system: &str, turns: &[Turn], tools: &[ToolDef]) -> HttpRequest {
    let mut input = Vec::new();
    for t in turns {
        match t {
            Turn::User(text) => input.push(json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]})),
            Turn::Assistant { native: Some((Api::Responses, items)), .. } => {
                input.extend(items.as_array().cloned().unwrap_or_default());
            }
            Turn::Assistant { text, calls, .. } => {
                if !text.trim().is_empty() {
                    input.push(json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]}));
                }
                for c in calls {
                    input.push(json!({"type": "function_call", "call_id": c.id, "name": c.name, "arguments": c.args.to_string()}));
                }
            }
            Turn::Results(rs) => {
                for r in rs {
                    input.push(json!({"type": "function_call_output", "call_id": r.id, "output": r.content}));
                }
            }
        }
    }
    let functions: Vec<Value> = tools
        .iter()
        .map(|t| json!({"type": "function", "name": t.name, "description": t.description, "parameters": t.schema, "strict": false}))
        .collect();
    let mut body = json!({
        "model": cfg.model.trim(),
        "instructions": system,
        "input": input,
        // A plan's requests stream and keep nothing server-side: the reasoning comes back
        // encrypted and is replayed with the conversation (`native`). The length of the reply
        // is the plan's to limit (`max_output_tokens` isn't accepted).
        "stream": true,
        "store": false,
    });
    // Older catalogs omit the capability; keep the existing behavior in that case.
    if cfg.reasoning != Some(false) {
        body["reasoning"] = json!({"effort": "medium"});
        body["include"] = json!(["reasoning.encrypted_content"]);
    }
    if !functions.is_empty() {
        body["tools"] = json!([{
            "type": "namespace",
            "name": TOOL_NAMESPACE,
            "description": "RustGIS: read and change the map, run commands and geoprocessing tools.",
            "tools": functions,
        }]);
        body["tool_choice"] = json!("auto");
        body["parallel_tool_calls"] = json!(true);
    }
    let mut headers = vec![header("Content-Type", "application/json"), header("Accept", "text/event-stream")];
    headers.extend(responses_headers(cfg));
    HttpRequest { method: "POST", url: format!("{}/responses", cfg.base()), headers, body: json_body(&body) }
}

/// The `data:` payloads of a server-sent event stream, parsed.
fn sse_events(body: &str) -> Vec<Value> {
    let mut events = Vec::new();
    let normalized = body.replace("\r\n", "\n");
    for block in normalized.split("\n\n") {
        let data: Vec<&str> = block.lines().filter_map(|l| l.strip_prefix("data:")).map(str::trim_start).collect();
        if data.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(&data.join("\n")) {
            events.push(v);
        }
    }
    events
}

/// Whether an output item can be replayed once its turn's calls are dropped: calls have no
/// outputs to pair with, and reasoning items are refused without the item that followed them.
fn replayable_without_calls(item: &Value) -> bool {
    !matches!(item.get("type").and_then(Value::as_str), Some("function_call" | "reasoning"))
}

/// Reads a streamed Responses reply, read to its end: the reply counts only once the stream says
/// `response.completed`. Incomplete replies retain text but cannot run tools. The output items are those
/// of the final `response`, or those the stream finished (`response.output_item.done`). A plain
/// JSON reply (a server that didn't stream) is read the same way.
fn responses_reply(resp: &HttpResponse) -> Result<Reply, Error> {
    let text = String::from_utf8_lossy(&resp.body);
    if !(200..300).contains(&resp.status) {
        let v = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
        return Err(Error::response(&v, resp));
    }
    let mut indexed_items: Vec<(u64, Value)> = Vec::new();
    let mut last: Option<Value> = None;
    let mut error: Option<Error> = None;
    let mut completed = false;
    let events = if text.trim_start().starts_with('{') {
        serde_json::from_str::<Value>(&text).ok().map(|v| vec![json!({"type": if v.get("status").and_then(Value::as_str) == Some("completed") { "response.completed" } else { "response.incomplete" }, "response": v})]).unwrap_or_default()
    } else {
        sse_events(&text)
    };
    for e in &events {
        match e.get("type").and_then(Value::as_str) {
            Some("response.output_item.done") => {
                if let Some(item) = e.get("item") {
                    let index = e.get("output_index").and_then(Value::as_u64).unwrap_or(indexed_items.len() as u64);
                    indexed_items.push((index, item.clone()));
                }
            }
            Some(kind @ ("response.completed" | "response.incomplete")) => {
                completed =
                    kind == "response.completed" && e.pointer("/response/status").and_then(Value::as_str).is_none_or(|s| s == "completed");
                last = e.get("response").cloned();
            }
            Some("response.failed") => {
                error = Some(Error::response(e, resp));
            }
            Some("error") => error = Some(Error::response(e, resp)),
            _ => {}
        }
    }
    if let Some(e) = error {
        return Err(e);
    }
    let last = last
        .ok_or_else(|| if events.is_empty() { http_error(resp) } else { "the stream ended before the reply was complete".to_string() })?;
    indexed_items.sort_by_key(|(index, _)| *index);
    let mut items: Vec<Value> = indexed_items.into_iter().map(|(_, item)| item).collect();
    if let Some(output) = last.get("output").and_then(Value::as_array).filter(|output| !output.is_empty()) {
        // The terminal response is authoritative and already ordered.
        items = output.clone();
    }
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut refused = None;
    for (i, item) in items.iter_mut().enumerate() {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                for c in item.get("content").and_then(Value::as_array).into_iter().flatten() {
                    match c.get("type").and_then(Value::as_str) {
                        Some("output_text") => text.push_str(c.get("text").and_then(Value::as_str).unwrap_or("")),
                        Some("refusal") => refused = Some(c.get("refusal").and_then(Value::as_str).unwrap_or("").to_string()),
                        _ => {}
                    }
                }
            }
            Some("function_call") => {
                // A call without an id gets one, kept on the replayed item so its output pairs.
                let id = match item.get("call_id").and_then(Value::as_str) {
                    Some(id) => id.to_string(),
                    None => {
                        let id = format!("call_{i}");
                        if let Some(o) = item.as_object_mut() {
                            o.insert("call_id".into(), json!(id));
                        }
                        id
                    }
                };
                let name = item.get("name").and_then(Value::as_str).unwrap_or("");
                calls.push(ToolCall {
                    id,
                    // Some models name a namespaced tool with its namespace.
                    name: name.strip_prefix(TOOL_NAMESPACE).and_then(|n| n.strip_prefix('.')).unwrap_or(name).to_string(),
                    args: match item.get("arguments") {
                        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| json!({"_unparsed": s})),
                        Some(other) => other.clone(),
                        None => json!({}),
                    },
                });
            }
            _ => {}
        }
    }
    let mut cut = match last.pointer("/incomplete_details/reason").and_then(Value::as_str) {
        Some("max_output_tokens") => Some("the reply reached its length limit".to_string()),
        Some("content_filter") => Some("the provider's content filter stopped the reply".to_string()),
        Some(r) => Some(format!("the reply stopped early ({r})")),
        None => None,
    };
    if let Some(r) = refused {
        cut = Some(if r.is_empty() {
            "the model declined this request".to_string()
        } else {
            format!("the model declined this request: {r}")
        });
        calls.clear();
        items.retain(replayable_without_calls);
    }
    if !completed {
        calls.clear();
        items.retain(replayable_without_calls);
        if cut.is_none() {
            cut = Some("The reply stopped before completion; no actions were run.".into());
        }
    }
    // Replayed as sent, minus the server's item ids (the conversation isn't stored there).
    let native: Vec<Value> = items
        .into_iter()
        .map(|mut item| {
            if let Some(o) = item.as_object_mut() {
                o.remove("id");
            }
            item
        })
        .collect();
    let failure = (!completed && last.get("error").is_some_and(Value::is_object)).then(|| Error::response(&last, resp));
    Ok(Reply { text, calls, native: Value::Array(native), cut, failure })
}

// ---------------------------------------------------------------------------------------------
// Anthropic Messages

/// Models that take the server-side refusal fallback (Claude API only).
fn takes_fallback(model: &str) -> bool {
    ["claude-opus-5", "claude-fable-5-1", "claude-sonnet-5-5"].iter().any(|m| model.starts_with(m))
}

fn anthropic_request(cfg: &Config, system: &str, turns: &[Turn], tools: &[ToolDef]) -> HttpRequest {
    let mut messages: Vec<Value> = Vec::new();
    // Consecutive turns of one role are merged into one message.
    let mut push = |role: &str, blocks: Vec<Value>| {
        if blocks.is_empty() {
            return;
        }
        if let Some(last) = messages.last_mut()
            && last["role"] == role
            && let Some(c) = last.get_mut("content").and_then(Value::as_array_mut)
        {
            c.extend(blocks);
            return;
        }
        messages.push(json!({"role": role, "content": blocks}));
    };
    for t in turns {
        match t {
            Turn::User(text) => push("user", vec![json!({"type": "text", "text": text})]),
            Turn::Assistant { native: Some((Api::Anthropic, content)), .. } => {
                push("assistant", content.as_array().cloned().unwrap_or_default());
            }
            Turn::Assistant { text, calls, .. } => {
                let mut blocks = Vec::new();
                if !text.trim().is_empty() {
                    blocks.push(json!({"type": "text", "text": text}));
                }
                for c in calls {
                    blocks.push(json!({"type": "tool_use", "id": c.id, "name": c.name, "input": object(&c.args)}));
                }
                push("assistant", blocks);
            }
            Turn::Results(rs) => push(
                "user",
                rs.iter().map(|r| json!({"type": "tool_result", "tool_use_id": r.id, "content": r.content, "is_error": r.error})).collect(),
            ),
        }
    }
    let tools: Vec<Value> = tools.iter().map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.schema})).collect();
    let model = cfg.model.trim();
    let mut body = json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "system": system,
        "messages": messages,
        "tools": tools,
        // Caches the system prompt, tools and history between turns.
        "cache_control": {"type": "ephemeral"},
    });
    let mut headers = vec![
        header("Content-Type", "application/json"),
        header("x-api-key", cfg.key.trim()),
        header("anthropic-version", "2023-06-01"),
        // Lets the browser app call the API directly (with the user's own key).
        header("anthropic-dangerous-direct-browser-access", "true"),
    ];
    // A request a model declines is retried on another (Claude API only).
    if cfg.base() == "https://api.anthropic.com" && takes_fallback(model) {
        body["fallbacks"] = json!("default");
        headers.push(header("anthropic-beta", "server-side-fallback-2026-07-01"));
    }
    HttpRequest { method: "POST", url: format!("{}/v1/messages", cfg.base()), headers, body: json_body(&body) }
}

fn anthropic_reply(v: &Value) -> Result<Reply, String> {
    let content = v.get("content").and_then(Value::as_array).ok_or("the reply has no content")?;
    let mut text = String::new();
    let mut calls = Vec::new();
    for b in content {
        match b.get("type").and_then(Value::as_str) {
            Some("text") => text.push_str(b.get("text").and_then(Value::as_str).unwrap_or("")),
            Some("tool_use") => calls.push(ToolCall {
                id: b.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                name: b.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                args: b.get("input").cloned().unwrap_or_else(|| json!({})),
            }),
            _ => {}
        }
    }
    let cut = match v.get("stop_reason").and_then(Value::as_str) {
        Some("max_tokens") => Some("the reply reached its length limit".to_string()),
        Some("refusal") => Some(
            v.pointer("/stop_details/explanation")
                .and_then(Value::as_str)
                .map_or_else(|| "the model declined this request".to_string(), |e| format!("the model declined this request: {e}")),
        ),
        _ => None,
    };
    // A refused reply's partial calls must not run.
    if v.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
        calls.clear();
    }
    Ok(Reply { text, calls, native: Value::Array(content.clone()), cut, failure: None })
}

// ---------------------------------------------------------------------------------------------
// Gemini

/// Gemini takes a subset of JSON Schema: an object needs properties, so a free-form object
/// parameter is sent as JSON text instead (the tools accept either).
fn gemini_schema(v: &Value) -> Value {
    match v {
        Value::Object(o) => {
            let is_object = o.get("type").and_then(Value::as_str) == Some("object");
            let has_props = o.get("properties").and_then(Value::as_object).is_some_and(|p| !p.is_empty());
            if is_object && !has_props {
                let desc = o.get("description").and_then(Value::as_str).unwrap_or("A JSON object");
                return json!({"type": "string", "description": format!("{desc} (as JSON text)")});
            }
            let mut out = Map::new();
            for (k, v) in o {
                if k == "additionalProperties" || k == "$schema" {
                    continue;
                }
                let v = if k == "properties" {
                    Value::Object(v.as_object().map(|p| p.iter().map(|(k, v)| (k.clone(), gemini_schema(v))).collect()).unwrap_or_default())
                } else if k == "items" {
                    gemini_schema(v)
                } else {
                    v.clone()
                };
                out.insert(k.clone(), v);
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// The prefix of call ids made up for Gemini calls that came without one.
const MADE_UP: &str = "rustgis-call-";

fn gemini_request(cfg: &Config, system: &str, turns: &[Turn], tools: &[ToolDef]) -> HttpRequest {
    let mut contents = Vec::new();
    for t in turns {
        match t {
            Turn::User(text) => contents.push(json!({"role": "user", "parts": [{"text": text}]})),
            Turn::Assistant { native: Some((Api::Gemini, c)), .. } => contents.push(c.clone()),
            Turn::Assistant { text, calls, .. } => {
                let mut parts = Vec::new();
                if !text.trim().is_empty() {
                    parts.push(json!({"text": text}));
                }
                for c in calls {
                    parts.push(json!({"functionCall": {"name": c.name, "args": object(&c.args)}}));
                }
                if !parts.is_empty() {
                    contents.push(json!({"role": "model", "parts": parts}));
                }
            }
            Turn::Results(rs) => {
                let parts: Vec<Value> = rs
                    .iter()
                    .map(|r| {
                        let response = if r.error { json!({"error": r.content}) } else { json!({"result": r.content}) };
                        let mut f = json!({"name": r.name, "response": response});
                        // Ids Gemini gave (not the ones made up for it) go back with the result.
                        if !r.id.starts_with(MADE_UP) {
                            f["id"] = json!(r.id);
                        }
                        json!({"functionResponse": f})
                    })
                    .collect();
                contents.push(json!({"role": "user", "parts": parts}));
            }
        }
    }
    let decls: Vec<Value> =
        tools.iter().map(|t| json!({"name": t.name, "description": t.description, "parameters": gemini_schema(&t.schema)})).collect();
    let body = json!({
        "systemInstruction": {"parts": [{"text": system}]},
        "contents": contents,
        "tools": [{"functionDeclarations": decls}],
    });
    let model = cfg.model.trim().trim_start_matches("models/");
    HttpRequest {
        method: "POST",
        url: format!("{}/models/{}:generateContent", cfg.base(), model),
        headers: vec![header("Content-Type", "application/json"), header("x-goog-api-key", cfg.key.trim())],
        body: json_body(&body),
    }
}

fn gemini_reply(v: &Value) -> Result<Reply, String> {
    let Some(cand) = v.get("candidates").and_then(|c| c.get(0)) else {
        let why = v.pointer("/promptFeedback/blockReason").and_then(Value::as_str).unwrap_or("no reason given");
        return Err(format!("Gemini sent no answer ({why})"));
    };
    let content = cand.get("content").cloned().unwrap_or_else(|| json!({"role": "model", "parts": []}));
    let mut text = String::new();
    let mut calls = Vec::new();
    for (i, p) in content.get("parts").and_then(Value::as_array).into_iter().flatten().enumerate() {
        if p.get("thought").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        if let Some(t) = p.get("text").and_then(Value::as_str) {
            text.push_str(t);
        }
        if let Some(f) = p.get("functionCall") {
            calls.push(ToolCall {
                id: f.get("id").and_then(Value::as_str).map_or_else(|| format!("{MADE_UP}{i}"), str::to_string),
                name: f.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                args: f.get("args").cloned().unwrap_or_else(|| json!({})),
            });
        }
    }
    let cut = match cand.get("finishReason").and_then(Value::as_str) {
        Some("MAX_TOKENS") => Some("the reply reached its length limit".to_string()),
        Some(r @ ("SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII")) => {
            Some(format!("Gemini stopped the reply ({r})"))
        }
        _ => None,
    };
    let mut native = content;
    if native.get("role").is_none() {
        native["role"] = json!("model");
    }
    Ok(Reply { text, calls, native, cut, failure: None })
}

/// `v` if it is an object, else an empty one (providers require objects for tool input).
fn object(v: &Value) -> Value {
    if v.is_object() { v.clone() } else { json!({}) }
}

// ---------------------------------------------------------------------------------------------
// Model lists

/// The request listing the provider's models.
pub fn models_request(cfg: &Config) -> HttpRequest {
    let key = cfg.key.trim();
    match cfg.api() {
        Api::OpenAi => {
            let mut headers = Vec::new();
            if !key.is_empty() {
                headers.push(header("Authorization", &format!("Bearer {key}")));
            }
            HttpRequest { method: "GET", url: format!("{}/models", cfg.base()), headers, body: None }
        }
        Api::Responses => HttpRequest { method: "GET", url: format!("{}/models", cfg.base()), headers: responses_headers(cfg), body: None },
        Api::Anthropic => HttpRequest {
            method: "GET",
            url: format!("{}/v1/models?limit=1000", cfg.base()),
            headers: vec![
                header("x-api-key", key),
                header("anthropic-version", "2023-06-01"),
                header("anthropic-dangerous-direct-browser-access", "true"),
            ],
            body: None,
        },
        Api::Gemini => HttpRequest {
            method: "GET",
            url: format!("{}/models?pageSize=1000", cfg.base()),
            headers: vec![header("x-goog-api-key", key)],
            body: None,
        },
    }
}

/// The model ids in a model list, sorted.
pub fn parse_models(api: Api, resp: &HttpResponse) -> Result<Vec<String>, String> {
    let v: Value = serde_json::from_slice(&resp.body).map_err(|_| http_error(resp))?;
    if !(200..300).contains(&resp.status) {
        return Err(error_message(&v).unwrap_or_else(|| http_error(resp)));
    }
    let mut ids: Vec<String> = match api {
        Api::OpenAi | Api::Anthropic => v
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| m.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .collect(),
        // A ChatGPT plan's catalog lists its models with a slug and whether they are shown.
        Api::Responses => v
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|m| m.get("visibility").and_then(Value::as_str) == Some("list"))
            .filter_map(|m| m.get("slug").and_then(Value::as_str))
            .map(str::to_string)
            .collect(),
        Api::Gemini => v
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|m| {
                m.get("supportedGenerationMethods")
                    .and_then(Value::as_array)
                    .is_some_and(|a| a.iter().any(|x| x.as_str() == Some("generateContent")))
            })
            .filter_map(|m| m.get("name").and_then(Value::as_str))
            .map(|n| n.trim_start_matches("models/").to_string())
            .collect(),
    };
    // The ChatGPT catalog comes in its own order (the first is the one to suggest).
    if api != Api::Responses {
        ids.sort();
        ids.dedup();
    }
    ids.truncate(2000);
    Ok(ids)
}

/// A model offered by the selected ChatGPT registration.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    /// The identifier sent to inference.
    pub id: String,
    /// The catalog's display name.
    pub name: String,
    /// Reasoning support, or unknown for older catalogs.
    pub reasoning: Option<bool>,
}

/// Reads the visible ChatGPT catalog, in server order.
pub fn parse_catalog(resp: &HttpResponse) -> Result<Vec<Model>, Error> {
    let v: Value = serde_json::from_slice(&resp.body).map_err(|_| Error::response(&Value::Null, resp))?;
    if !(200..300).contains(&resp.status) {
        return Err(Error::response(&v, resp));
    }
    Ok(v.get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|m| m.get("visibility").and_then(Value::as_str) == Some("list"))
        .filter_map(|m| {
            let id = m.get("slug")?.as_str()?;
            let reasoning = m
                .get("supports_reasoning")
                .and_then(Value::as_bool)
                .or_else(|| m.get("supported_reasoning_levels").and_then(Value::as_array).map(|a| !a.is_empty()));
            Some(Model { id: id.into(), name: m.get("display_name").and_then(Value::as_str).unwrap_or(id).into(), reasoning })
        })
        .take(2000)
        .collect())
}

/// Uses the first offered model when the current one is absent, and applies its capabilities.
pub fn use_catalog(cfg: &mut Config, models: &[Model], explicit: bool) {
    let selected = models.iter().find(|m| m.id == cfg.model).or_else(|| if explicit { None } else { models.first() });
    cfg.reasoning = selected.and_then(|m| m.reasoning);
    if let Some(model) = selected {
        cfg.model = model.id.clone();
    }
}
