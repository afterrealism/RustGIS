//! A Model Context Protocol server: RustGIS for any MCP client (Claude Code, Claude Desktop,
//! Cursor, VS Code…). The client's model drives a session through the assistant's tools
//! ([`crate::tools`]), sees the map as an image and reads the built-in documentation
//! ([`crate::docs`]).
//!
//! Sans I/O: [`Server::handle`] takes one JSON-RPC message and gives back the reply, so
//! `rustgis-cli mcp` (standard input and output) and the tests drive the same code.
//!
//! The client asks its user before each tool call; the server adds its own floor. File and
//! network access is refused unless the server was started allowing it, and the user's own
//! automation (scripts, tasks, models, plugin management) is never run. Without access, the only
//! network use left is drawing web tile layers that the project the user opened already has.

use rustgis_engine::Session;
use rustgis_engine::cmd::plugins::{Reach, reaches, user_only};
use serde_json::{Value, json};

use crate::provider::ToolCall;
use crate::{docs, tools};

/// The protocol versions this server speaks, newest first.
pub const VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// The most messages one batch may hold.
pub const MAX_BATCH: usize = 16;

/// The largest map image `view_map` draws (pixels a side).
const MAX_VIEW: f64 = 2048.0;

/// What the model is told about the server when it connects.
const INSTRUCTIONS: &str = "RustGIS is a desktop GIS. This server holds one RustGIS project in memory and acts on it.\n\
- list_layers shows the map. view_map shows it as an image: look after changing it.\n\
- Every action is a command: search_commands finds one, run_command runs it (navigation, layers, symbology, labels, selection, editing, layouts, export…).\n\
- Geoprocessing: search_tools, then describe_tool for its parameters, then run_tool; its output is a new layer.\n\
- read_features reads attribute rows, with a where clause.\n\
- search_docs and read_doc look things up in the RustGIS documentation.\n\
Open data with the layer.addData command ({\"path\": …}) and save with project.saveAs ({\"path\": \"*.rgis\"}); \
file and network access works only when the server was started with --yes.";

/// The server: the session's access policy. The session itself is the caller's.
#[derive(Debug, Clone, Default)]
pub struct Server {
    /// Whether commands may read and write files, use the network and run plugin commands.
    pub allow_access: bool,
}

/// A JSON-RPC error: its code and message.
type RpcError = (i64, String);

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
/// MCP's code for a resource that doesn't exist.
const RESOURCE_NOT_FOUND: i64 = -32002;

/// Commands that replace the project or step through its history. They are reserved to the user
/// in the desktop app, to keep their work; here the session is the client's own.
const PROJECT_COMMANDS: &[&str] = &["project.new", "project.open", "project.openText", "project.sample", "project.fromTemplate"];

impl Server {
    pub fn new(allow_access: bool) -> Self {
        Self { allow_access }
    }

    /// Handles one message (a line of JSON): the reply to send, or `None` for a notification or
    /// a response.
    pub fn handle(&self, s: &mut Session, message: &str) -> Option<String> {
        let msg: Value = match serde_json::from_str(message) {
            Ok(v) => v,
            Err(e) => return Some(error(&Value::Null, PARSE_ERROR, &format!("not JSON: {e}"))),
        };
        let reply = match &msg {
            // A batch (protocol 2025-03-26): one reply per request, none when all are notifications.
            Value::Array(batch) if batch.is_empty() => error_value(&Value::Null, INVALID_REQUEST, "empty batch"),
            // Each reply is held until the batch is done, and a map image can be megabytes.
            Value::Array(batch) if batch.len() > MAX_BATCH => {
                error_value(&Value::Null, INVALID_REQUEST, &format!("a batch holds at most {MAX_BATCH} messages"))
            }
            Value::Array(batch) => {
                let replies: Vec<Value> = batch.iter().filter_map(|m| self.handle_value(s, m)).collect();
                if replies.is_empty() {
                    return None;
                }
                Value::Array(replies)
            }
            _ => self.handle_value(s, &msg)?,
        };
        Some(reply.to_string())
    }

    fn handle_value(&self, s: &mut Session, msg: &Value) -> Option<Value> {
        let Some(obj) = msg.as_object() else {
            return Some(error_value(&Value::Null, INVALID_REQUEST, "expected a JSON-RPC object"));
        };
        let id = obj.get("id").cloned();
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            // A response to a request of ours (we send none), or junk.
            return id
                .filter(|_| !obj.contains_key("result") && !obj.contains_key("error"))
                .map(|id| error_value(&id, INVALID_REQUEST, "expected a method"));
        };
        // Notifications (no id) get no reply.
        let id = id?;
        if !matches!(id, Value::String(_) | Value::Number(_)) {
            return Some(error_value(&Value::Null, INVALID_REQUEST, "id must be a string or a number"));
        }
        let params = obj.get("params").cloned().unwrap_or(Value::Null);
        Some(match self.call(s, method, &params) {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => error_value(&id, code, &message),
        })
    }

    fn call(&self, s: &mut Session, method: &str, params: &Value) -> Result<Value, RpcError> {
        match method {
            "initialize" => Ok(initialize(params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tool_list()})),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).ok_or((INVALID_PARAMS, "name is required".to_string()))?;
                let args = match params.get("arguments") {
                    None | Some(Value::Null) => json!({}),
                    Some(v @ Value::Object(_)) => v.clone(),
                    Some(_) => return Err((INVALID_PARAMS, "arguments must be an object".into())),
                };
                if !TOOL_NAMES.contains(&name) && !tools::defs().iter().any(|d| d.name == name) {
                    return Err((INVALID_PARAMS, format!("unknown tool '{name}'")));
                }
                Ok(self.call_tool(s, name, args))
            }
            "resources/list" => Ok(json!({"resources": resource_list()})),
            "resources/templates/list" => Ok(json!({"resourceTemplates": []})),
            "resources/read" => {
                let uri = params.get("uri").and_then(Value::as_str).ok_or((INVALID_PARAMS, "uri is required".to_string()))?;
                let page =
                    uri.strip_prefix("rustgis://docs/").and_then(docs::page).ok_or((RESOURCE_NOT_FOUND, format!("no resource '{uri}'")))?;
                Ok(json!({"contents": [{"uri": uri, "mimeType": "text/markdown", "text": page.text}]}))
            }
            "prompts/list" => Ok(json!({"prompts": []})),
            _ => Err((METHOD_NOT_FOUND, format!("unknown method '{method}'"))),
        }
    }

    /// Runs a tool; failures are results the model reads (`isError`), not protocol errors.
    fn call_tool(&self, s: &mut Session, name: &str, args: Value) -> Value {
        let text = |t: String, is_error: bool| json!({"content": [{"type": "text", "text": t}], "isError": is_error});
        let result = match name {
            "view_map" => return view_map(s, &args).unwrap_or_else(|e| text(e, true)),
            _ => {
                let call = ToolCall { id: String::new(), name: name.to_string(), args };
                match self.refusal(s, &call) {
                    Some(why) => Err(why),
                    None => tools::run(s, &call),
                }
            }
        };
        match result {
            Ok(t) => text(t, false),
            Err(e) => text(e, true),
        }
    }

    /// Why this server won't run a call, if it won't.
    pub fn refusal(&self, s: &Session, call: &ToolCall) -> Option<String> {
        let (id, params) = tools::target(call)?;
        if user_only(&id) && !PROJECT_COMMANDS.contains(&id.as_str()) && !matches!(id.as_str(), "edit.undo" | "edit.redo") {
            return Some(format!(
                "'{id}' is reserved to the user (plugin management, scripts, tasks, models and the GeoLibre runner); run it in RustGIS itself"
            ));
        }
        let why = if id.starts_with(rustgis_engine::plugins::PREFIX) {
            "it runs a plugin's command"
        } else if id == "project.openText" {
            // A project can hold web tile layers, which drawing the map (view_map) fetches.
            "a project can bring in web layers that connect to the internet"
        } else {
            match reaches(s.registry.get(&id)?, &params)? {
                Reach::Files => "it reads or writes files on this computer",
                Reach::Network => "it connects to an address on the internet",
            }
        };
        (!self.allow_access)
            .then(|| format!("'{id}' was not run: {why}. The server allows that only when started with --yes (rustgis-cli mcp --yes)."))
    }
}

fn initialize(params: &Value) -> Value {
    // Speak the client's version when we know it, else our newest.
    let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
    let version = VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {"listChanged": false}, "resources": {"listChanged": false}},
        "serverInfo": {"name": "rustgis", "title": "RustGIS", "version": env!("CARGO_PKG_VERSION")},
        "instructions": INSTRUCTIONS,
    })
}

/// The tools this server adds to the assistant's.
const TOOL_NAMES: &[&str] = &["view_map"];

/// Tools that only look: list, search, read, view.
fn read_only(name: &str) -> bool {
    !matches!(name, "run_command" | "run_tool")
}

fn tool_list() -> Vec<Value> {
    let mut out: Vec<Value> = tools::defs()
        .into_iter()
        .map(|d| {
            let description = match d.name {
                "run_command" => "Run a RustGIS command by id with JSON parameters (see search_commands for ids and parameters). Each run is one undo step (edit.undo undoes it). Commands that read or write files or use the network run only when the server was started with --yes.".to_string(),
                "read_doc" => {
                    let pages: Vec<&str> = docs::PAGES.iter().map(|p| p.path).collect();
                    format!("Read a RustGIS documentation page or section as Markdown. Use next_offset to continue budgeted results. Pages: {}.", pages.join(", "))
                }
                _ => d.description.to_string(),
            };
            json!({"name": d.name, "description": description, "inputSchema": d.schema, "annotations": annotations(d.name)})
        })
        .collect();
    out.push(json!({
        "name": "view_map",
        "description": "The map as it looks now, as a PNG image: look after changing it. The current view, at a size of your choosing (default 1024 x 640).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "width": {"type": "integer", "description": "Pixels (64 to 2048)"},
                "height": {"type": "integer", "description": "Pixels (64 to 2048)"}
            }
        },
        "annotations": annotations("view_map"),
    }));
    out
}

fn annotations(name: &str) -> Value {
    let ro = read_only(name);
    // Commands can delete features, overwrite files and replace the project.
    json!({"readOnlyHint": ro, "destructiveHint": !ro, "idempotentHint": ro, "openWorldHint": !ro})
}

fn resource_list() -> Vec<Value> {
    docs::PAGES
        .iter()
        .map(|p| {
            json!({
                "uri": format!("rustgis://docs/{}", p.path),
                "name": p.path,
                "title": p.title(),
                "description": format!("RustGIS documentation: {} ({})", p.title(), p.url()),
                "mimeType": "text/markdown",
            })
        })
        .collect()
}

fn view_map(s: &mut Session, a: &Value) -> Result<Value, String> {
    use base64::Engine;
    let size = |k: &str, default: f64| a.get(k).and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(default).clamp(64.0, MAX_VIEW);
    let (w, h) = (size("width", 1024.0), size("height", 640.0));
    let png = rustgis_engine::cmd::share::map_png(s, Some(w), Some(h), None)?;
    let m = s.map();
    let c = m.center_lonlat();
    let caption = format!(
        "Map \"{}\", {} layers, centre {:.4}, {:.4} (lon, lat), scale 1:{:.0}, {w:.0} x {h:.0} px.",
        m.name,
        m.layers.len(),
        c.x,
        c.y,
        m.scale()
    );
    Ok(json!({
        "content": [
            {"type": "image", "data": base64::engine::general_purpose::STANDARD.encode(png), "mimeType": "image/png"},
            {"type": "text", "text": caption},
        ],
        "isError": false,
    }))
}

/// Whether the server answers this message: a request (an id, not a response to one) or an
/// unparseable line. A transport that waits for the server's reply — the relay client — needs
/// this to know a notification gets none.
pub fn expects_reply(message: &str) -> bool {
    // A non-object is an INVALID_REQUEST the server answers (`handle_value`), so it expects a
    // reply too; an object answers when it has an id and is neither a result nor an error.
    let replies = |m: &Value| match m.as_object() {
        None => true,
        Some(o) => o.contains_key("id") && !o.contains_key("result") && !o.contains_key("error"),
    };
    match serde_json::from_str::<Value>(message) {
        Ok(Value::Array(batch)) => batch.is_empty() || batch.iter().any(replies),
        Ok(v) => replies(&v),
        // Not JSON: the server answers with a parse error.
        Err(_) => true,
    }
}

fn error_value(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn error(id: &Value, code: i64, message: &str) -> String {
    error_value(id, code, message).to_string()
}
