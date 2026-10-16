use std::collections::VecDeque;

use rustgis_engine::{Project, Session};
use serde_json::{Value, json};

use crate::agent::{Agent, Entry, INTERNAL_ERROR, MAX_STEPS, Step, run_blocking};
use crate::chatgpt;
use crate::provider::*;
use crate::tools;

fn sample() -> Session {
    let mut s = Session::new(Project::default());
    let _ = s.run("project.sample", &Value::Null);
    s
}

fn cfg(provider: &str) -> Config {
    let p = preset(provider).unwrap_or(&PRESETS[0]);
    Config {
        provider: provider.into(),
        base_url: p.base_url.into(),
        model: if p.model.is_empty() { "m".into() } else { p.model.into() },
        key: "k".into(),
        account: String::new(),
        client_id: String::new(),
        session: String::new(),
        reasoning: None,
    }
}

fn ok(v: Value) -> Result<HttpResponse, String> {
    Ok(HttpResponse { status: 200, body: v.to_string().into_bytes(), request_id: None })
}

/// An OpenAI reply calling one tool.
fn call(id: &str, name: &str, args: Value) -> Value {
    json!({"choices": [{"message": {"role": "assistant", "content": null,
        "tool_calls": [{"id": id, "type": "function", "function": {"name": name, "arguments": args.to_string()}}]},
        "finish_reason": "tool_calls"}]})
}

fn say(text: &str) -> Value {
    json!({"choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]})
}

fn body(req: &HttpRequest) -> Value {
    serde_json::from_str(req.body.as_deref().unwrap_or("null")).unwrap_or(Value::Null)
}

#[test]
fn urls_allowed() {
    assert!(allowed_url("https://api.openai.com/v1/chat/completions"));
    assert!(allowed_url("http://localhost:11434/v1/models"));
    assert!(allowed_url("http://127.0.0.1:1234/v1"));
    assert!(allowed_url("http://192.168.1.20:11434/v1"));
    assert!(allowed_url("http://[::1]:8080/v1"));
    assert!(allowed_url("http://172.20.0.2/v1"));
    assert!(!allowed_url("http://example.com/v1"));
    assert!(!allowed_url("http://localhost@example.com/v1"));
    assert!(!allowed_url("http://172.40.0.2/v1"));
    assert!(!allowed_url("http://10.example.com/v1"));
    assert!(!allowed_url("file:///etc/passwd"));
    assert!(!allowed_url("https://user:key@api.example.com/v1"));
    assert!(!allowed_url("https://"));
    assert!(!allowed_url("http://010.0.0.1/v1"));
    assert!(!allowed_url("http://0177.0.0.1/v1"));
    assert!(allowed_url("http://10.0.0.1/v1"));
    assert!(!allowed_url(""));
}

#[test]
fn openai_round_trip() {
    let s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(req) = a.ask("Zoom to Paris", &s) else { panic!("expected a request") };
    assert_eq!(req.url, "https://api.openai.com/v1/chat/completions");
    assert!(req.headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer k"));
    let b = body(&req);
    assert_eq!(b["messages"][0]["role"], "system");
    let user = b["messages"][1]["content"].as_str().unwrap_or("");
    assert!(user.contains("<map>") && user.contains("\"Cities\"") && user.ends_with("Zoom to Paris"));
    assert!(b["tools"].as_array().is_some_and(|t| t.iter().any(|t| t["function"]["name"] == "run_command")));
}

#[test]
fn agent_runs_commands_and_answers() {
    let mut s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let mut replies: VecDeque<Value> = VecDeque::from(vec![
        call("c1", "run_command", json!({"command": "map.goToXY", "params": {"lon": 2.35, "lat": 48.85, "scale": 5000000}})),
        say("Done: the map is centred on **Paris**."),
    ]);
    let mut requests = Vec::new();
    run_blocking(
        &mut a,
        "Zoom to Paris",
        &mut s,
        |req| {
            requests.push(body(req));
            replies.pop_front().map(ok).unwrap_or_else(|| Err("no more replies".into()))
        },
        |_, _, _| true,
    );
    let c = s.map().center_lonlat();
    assert!((c.x - 2.35).abs() < 0.01 && (c.y - 48.85).abs() < 0.01, "map moved to {c:?}");
    assert!(matches!(a.transcript.get(1), Some(Entry::Tool { error: false, .. })), "{:?}", a.transcript);
    assert!(matches!(a.transcript.last(), Some(Entry::Assistant(t)) if t.contains("Paris")));
    // The second request carries the call and its result.
    let msgs = requests.get(1).and_then(|b| b["messages"].as_array().cloned()).unwrap_or_default();
    assert!(msgs.iter().any(|m| m["role"] == "tool" && m["tool_call_id"] == "c1"));
    assert!(!a.busy());
    // The context is sent again only when the map changed: it did (the camera moved).
    let Step::Send(req) = a.ask("And now?", &s) else { panic!("expected a request") };
    let b = body(&req);
    let last = b["messages"].as_array().and_then(|m| m.last().cloned()).unwrap_or(Value::Null);
    assert!(last["content"].as_str().is_some_and(|t| t.starts_with("<map>")));
}

#[test]
fn unchanged_context_is_not_repeated() {
    let s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(_) = a.ask("hi", &s) else { panic!("expected a request") };
    let mut s2 = sample();
    let _ = a.receive(a.generation(), ok(say("Hello")), &mut s2);
    let Step::Send(req) = a.ask("what layers?", &s) else { panic!("expected a request") };
    let b = body(&req);
    let last = b["messages"].as_array().and_then(|m| m.last().cloned()).unwrap_or(Value::Null);
    assert_eq!(last["content"], "what layers?");
}

#[test]
fn file_access_needs_approval() {
    let mut s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(_) = a.ask("export the map", &s) else { panic!("expected a request") };
    let path = std::env::temp_dir().join("rustgis-assistant-never-written.png");
    let step =
        a.receive(a.generation(), ok(call("c1", "run_command", json!({"command": "share.exportMap", "params": {"path": path}}))), &mut s);
    assert_eq!(step, Step::Confirm);
    let (what, why, _) = a.pending(&s).unwrap_or_default();
    assert!(what.starts_with("share.exportMap") && why.contains("files"), "{what} / {why}");
    let Step::Send(req) = a.decide(false, false, &mut s) else { panic!("expected a request") };
    assert!(!path.exists());
    let b = body(&req);
    let tool_msg = b["messages"].as_array().and_then(|m| m.iter().find(|m| m["role"] == "tool").cloned()).unwrap_or(Value::Null);
    assert!(tool_msg["content"].as_str().is_some_and(|t| t.contains("declined")));
}

#[test]
fn gate_rules() {
    let s = sample();
    let g = |name: &str, args: Value| tools::gate(&s, &ToolCall { id: "x".into(), name: name.into(), args });
    assert!(g("run_command", json!({"command": "map.zoomFull"})).is_none());
    assert!(g("run_command", json!({"command": "edit.undo"})).is_none());
    assert!(g("run_command", json!({"command": "layer.addData", "params": {"path": "/etc/passwd"}})).is_some());
    assert!(g("run_command", json!({"command": "project.new"})).is_some());
    assert!(g("run_command", json!({"command": "script.run", "params": {"script": "map.zoomFull {}"}})).is_some());
    assert!(g("run_command", json!({"command": "plugin.install", "params": {"path": "x.rgplugin"}})).is_some());
    assert!(g("run_command", json!({"command": "map.addTiles", "params": {"url": "https://tile.example/{z}/{x}/{y}.png"}})).is_some());
    assert!(g("run_command", json!({"command": "plugin:x/y"})).is_some());
    let sql = g("run_command", json!({"command": "layer.addSql", "params": {"sql": "SELECT ST_Point(0, 0) AS g"}}));
    assert!(sql.as_deref().is_some_and(|w| w.contains("SQL") && w.contains("internet")), "{sql:?}");
    let sql = g("run_command", json!({"command": "layer.addSql", "params": {"sql": "FROM t", "database": "/data/x.duckdb"}}));
    assert!(sql.as_deref().is_some_and(|w| w.contains("views in the database /data/x.duckdb")), "{sql:?}");
    assert!(g("run_tool", json!({"tool": "buffer", "params": {"input": "Cities", "distance": 100}})).is_none());
    assert!(g("list_layers", json!({})).is_none());
    assert!(g("run_command", json!({"command": "info.fileLayers", "params": {"path": "/etc/passwd"}})).is_some());
    assert!(g("run_command", json!({"command": "info.fileColumns", "params": {"path": "/etc/passwd"}})).is_some());
}

/// What a command says beside its result reaches the model (issue #85).
#[test]
fn tool_results_carry_notices() {
    let mut s = sample();
    let call = ToolCall {
        id: "x".into(),
        name: "run_command".into(),
        args: json!({"command": "layer.addDataText", "params": {"fileName": "pts.csv", "text": "lon,lat\n1,2\nx,y\n"}}),
    };
    let out: Value = serde_json::from_str(&tools::run(&mut s, &call).unwrap()).unwrap();
    assert!(
        out["notices"].as_array().is_some_and(|n| n.iter().any(|t| t.as_str().is_some_and(|t| t.contains("no usable coordinates")))),
        "{out}"
    );
    let quiet: Value = serde_json::from_str(
        &tools::run(&mut s, &ToolCall { id: "y".into(), name: "run_command".into(), args: json!({"command": "map.zoomFull"}) }).unwrap(),
    )
    .unwrap();
    assert!(quiet.get("notices").is_none(), "{quiet}");
}

/// map.activate / map.remove must not report the newly visible map's layers as layersAdded (#212).
#[test]
fn map_switch_and_remove_do_not_report_layers_added() {
    let mut s = sample();
    let t = |name: &str, args: Value| ToolCall { id: "x".into(), name: name.into(), args };
    let world = s.map().name.clone();
    assert!(!world.is_empty(), "sample project needs a named map");
    let on_world: Value = serde_json::from_str(
        &tools::run(&mut s, &t("run_command", json!({"command": "map.activate", "params": {"name": world}}))).unwrap(),
    )
    .unwrap();
    assert!(on_world.get("layersAdded").is_none(), "activating the current map must not invent layersAdded: {on_world}");

    tools::run(&mut s, &t("run_command", json!({"command": "map.new", "params": {"name": "Repro"}}))).unwrap();
    assert_eq!(s.map().name, "Repro");
    let activate: Value = serde_json::from_str(
        &tools::run(&mut s, &t("run_command", json!({"command": "map.activate", "params": {"name": world}}))).unwrap(),
    )
    .unwrap();
    assert!(activate.get("layersAdded").is_none(), "map.activate must not report the target map's layers as added: {activate}");
    assert_eq!(s.map().name, world);

    let remove: Value = serde_json::from_str(
        &tools::run(&mut s, &t("run_command", json!({"command": "map.remove", "params": {"name": "Repro"}}))).unwrap(),
    )
    .unwrap();
    assert!(remove.get("layersAdded").is_none(), "map.remove must not report remaining layers as added: {remove}");
    assert_eq!(s.project.maps.len(), 1);

    // Removing the active, populated map switches to another map's layers: still nothing added.
    tools::run(&mut s, &t("run_command", json!({"command": "map.new", "params": {"name": "Gone"}}))).unwrap();
    let pts = json!({"fileName": "pts.csv", "text": "lon,lat\n1,2\n"});
    tools::run(&mut s, &t("run_command", json!({"command": "layer.addDataText", "params": pts.clone()}))).unwrap();
    assert_eq!(s.map().name, "Gone");
    let remove_active: Value =
        serde_json::from_str(&tools::run(&mut s, &t("run_command", json!({"command": "map.remove", "params": {"name": "Gone"}}))).unwrap())
            .unwrap();
    assert!(remove_active.get("layersAdded").is_none(), "removing the active map must not report the next map's layers: {remove_active}");
    assert_eq!(s.map().name, world);

    // Renaming the active map keeps its layers: nothing added.
    let renamed: Value = serde_json::from_str(
        &tools::run(&mut s, &t("run_command", json!({"command": "map.rename", "params": {"name": "Renamed"}}))).unwrap(),
    )
    .unwrap();
    assert!(renamed.get("layersAdded").is_none(), "map.rename must not report the map's layers as added: {renamed}");

    // A layer added to a map the command also created is still reported.
    tools::run(&mut s, &t("run_command", json!({"command": "map.new", "params": {"name": "Fresh"}}))).unwrap();
    let fresh: Value =
        serde_json::from_str(&tools::run(&mut s, &t("run_command", json!({"command": "layer.addDataText", "params": pts}))).unwrap())
            .unwrap();
    assert!(fresh.get("layersAdded").and_then(Value::as_array).is_some_and(|a| a.len() == 1), "{fresh}");
    tools::run(&mut s, &t("run_command", json!({"command": "map.activate", "params": {"name": "Renamed"}}))).unwrap();

    // Real adds still report layersAdded.
    let buffered: Value = serde_json::from_str(
        &tools::run(&mut s, &t("run_tool", json!({"tool": "buffer", "params": {"input": "Cities", "distance": 100}}))).unwrap(),
    )
    .unwrap();
    let added = buffered.get("layersAdded").and_then(Value::as_array);
    assert!(added.is_some_and(|a| !a.is_empty()), "buffer must name the layer it added: {buffered}");
}

#[test]
fn geoprocessing_and_reading_data() {
    let mut s = sample();
    let t = |name: &str, args: Value| ToolCall { id: "x".into(), name: name.into(), args };
    let found = tools::run(&mut s, &t("search_tools", json!({"query": "buffer"}))).unwrap_or_default();
    assert!(found.contains("\"buffer\""), "{found}");
    let desc = tools::run(&mut s, &t("describe_tool", json!({"tool": "buffer"}))).unwrap_or_default();
    assert!(desc.contains("distance"), "{desc}");
    let before = s.map().layers.len();
    let ran = tools::run(&mut s, &t("run_tool", json!({"tool": "buffer", "params": {"input": "Cities", "distance": 100}})));
    assert!(ran.as_deref().is_ok_and(|r| r.contains("layersAdded")), "{ran:?}");
    assert_eq!(s.map().layers.len(), before + 1);
    let rows = tools::run(
        &mut s,
        &t("read_features", json!({"layer": "Cities", "where": "pop_max > 10000000", "fields": ["name", "pop_max"], "order_by": "pop_max", "descending": true, "limit": 3})),
    )
    .unwrap_or_default();
    let v: Value = serde_json::from_str(&rows).unwrap_or(Value::Null);
    assert!(v["matched"].as_u64().is_some_and(|n| n > 3), "{rows}");
    let r = v["rows"].as_array().cloned().unwrap_or_default();
    assert_eq!(r.len(), 3);
    assert!(r[0]["pop_max"].as_i64() >= r[1]["pop_max"].as_i64());
    assert!(tools::run(&mut s, &t("read_features", json!({"layer": "Cities", "fields": ["nope"]}))).is_err());
    assert!(tools::run(&mut s, &t("read_features", json!({"layer": "Cities", "fields": [1, null]}))).is_err());
    let cmds = tools::run(&mut s, &t("search_commands", json!({"query": "basemap"}))).unwrap_or_default();
    assert!(cmds.contains("map.basemap"), "{cmds}");
}

#[test]
fn hostile_arguments_never_panic() {
    let mut s = sample();
    let junk = [
        Value::Null,
        json!({}),
        json!([1, 2]),
        json!("text"),
        json!({"command": 5, "params": "not json", "tool": [], "layer": {"a": 1}, "query": null, "limit": -4}),
        json!({"command": "map.goToXY", "params": "{\"lon\": 1e309}"}),
        json!({"command": "ui.about"}),
        json!({"command": "nope.nope"}),
        json!({"tool": "nope", "params": {"input": "Nope"}}),
        json!({"layer": "Cities", "where": "pop_max >", "limit": 1e20, "order_by": "", "fields": [1, null]}),
        json!({"layer": 99999, "selected": true}),
        json!({"query": "∑≈ç√ 🙂 a b c d e f g h i j k l m n o p", "limit": 100000}),
        json!({"tool": "buffer", "params": {"input": "Cities", "distance": "far"}}),
    ];
    for def in tools::defs() {
        for args in &junk {
            let c = ToolCall { id: "x".into(), name: def.name.into(), args: args.clone() };
            let _ = tools::gate(&s, &c);
            let _ = tools::summary(&c);
            if let Ok(r) = tools::run(&mut s, &c) {
                assert!(r.chars().count() <= tools::MAX_RESULT + 60);
            }
        }
    }
    let _ = tools::run(&mut s, &ToolCall { id: "x".into(), name: "rm_rf".into(), args: Value::Null });
}

#[test]
fn hostile_replies_never_panic() {
    let bodies = [
        "",
        "null",
        "[]",
        "{\"choices\": []}",
        "{\"choices\": [{\"message\": {\"tool_calls\": [{\"function\": {\"arguments\": \"{oops\"}}]}}]}",
        "{\"content\": [{\"type\": \"tool_use\"}], \"stop_reason\": \"refusal\"}",
        "{\"candidates\": [{\"content\": {\"parts\": [{\"functionCall\": {}}, {\"text\": 5}]}}]}",
        "{\"error\": {\"message\": \"Invalid API key\"}}",
        "<html>bad gateway</html>",
        "data: null\n\n",
        "data: {not json}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":[]}\n\n",
        "data: {\"type\":\"response.incomplete\",\"response\":{\"output\":[null,{},true]}}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"arguments\":false}}\n\n",
    ];
    for api in [Api::OpenAi, Api::Responses, Api::Anthropic, Api::Gemini] {
        for b in bodies {
            for status in [200, 401, 500] {
                let _ = parse_reply(api, &HttpResponse { status, body: b.as_bytes().to_vec(), request_id: None });
                let _ = parse_models(api, &HttpResponse { status, body: b.as_bytes().to_vec(), request_id: None });
            }
        }
    }
    let e = parse_reply(
        Api::OpenAi,
        &HttpResponse { status: 401, body: b"{\"error\": {\"message\": \"Invalid API key\"}}".to_vec(), request_id: None },
    );
    assert_eq!(e.err().map(|e| e.to_string()).as_deref(), Some("Invalid API key"));
    // A bad reply ends the run with an error the user sees.
    let mut s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(_) = a.ask("hi", &s) else { panic!("expected a request") };
    assert_eq!(a.receive(a.generation(), Err("offline".into()), &mut s), Step::Done);
    assert!(matches!(a.transcript.last(), Some(Entry::Error(e)) if e.to_string() == "offline"));
    assert!(!a.busy());
}

#[test]
fn a_panic_while_handling_a_reply_ends_the_turn_instead_of_the_app() {
    // Issue #110: the desktop app quit while the assistant handled a reply. A panic in a step
    // now becomes an error in the transcript, and the conversation stays usable.
    let mut s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(_) = a.ask("label the cities", &s) else { panic!("expected a request") };
    // Two calls: the first panics while it runs, the second is still queued.
    let two = json!({"choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [
        {"id": "c1", "type": "function", "function": {"name": "test_panic", "arguments": "{}"}},
        {"id": "c2", "type": "function", "function": {"name": "search_commands", "arguments": "{\"query\": \"label\"}"}}]},
        "finish_reason": "tool_calls"}]});
    let step = a.receive(a.generation(), ok(two), &mut s);
    assert_eq!(step, Step::Done);
    assert!(!a.busy());
    assert!(a.transcript.iter().any(|e| matches!(e, Entry::Error(m) if m.to_string() == INTERNAL_ERROR)));
    assert!(a.transcript.iter().any(|e| matches!(e, Entry::Note(m) if m.contains("a test tool panicked"))));
    // The next request answers the call that failed, so the provider accepts it.
    let Step::Send(req) = a.ask("try again", &s) else { panic!("expected a request") };
    let b = body(&req);
    let messages = b["messages"].as_array().cloned().unwrap_or_default();
    let result = |id: &str| {
        messages
            .iter()
            .find(|m| m["role"] == "tool" && m["tool_call_id"] == id)
            .and_then(|m| m["content"].as_str())
            .unwrap_or("")
            .to_string()
    };
    // The call that was running may have done something; the queued one never ran.
    assert!(result("c1").contains("may have partly taken effect"), "{messages:?}");
    assert!(result("c2").starts_with("Not run"), "{messages:?}");
    // Results go back in the order of the calls (Gemini matches id-less calls by order).
    let ids: Vec<&str> = messages.iter().filter(|m| m["role"] == "tool").filter_map(|m| m["tool_call_id"].as_str()).collect();
    assert_eq!(ids, ["c1", "c2"]);
    // A reply after the failure is read as usual.
    assert_eq!(a.receive(a.generation(), ok(say("Done.")), &mut s), Step::Done);
    assert!(matches!(a.transcript.last(), Some(Entry::Assistant(t)) if t == "Done."));
}

#[test]
fn anthropic_format() {
    let mut s = sample();
    let mut a = Agent::new(cfg("anthropic"), &s);
    let Step::Send(req) = a.ask("list layers", &s) else { panic!("expected a request") };
    assert_eq!(req.url, "https://api.anthropic.com/v1/messages");
    assert!(req.headers.iter().any(|(k, v)| k == "x-api-key" && v == "k"));
    assert!(req.headers.iter().any(|(k, _)| k == "anthropic-version"));
    let b = body(&req);
    assert!(b["system"].as_str().is_some_and(|t| t.contains("RustGIS")));
    assert_eq!(b["fallbacks"], "default");
    assert!(b["tools"][0]["input_schema"].is_object());
    let reply = json!({"content": [
        {"type": "thinking", "thinking": "", "signature": "sig"},
        {"type": "text", "text": "Let me look."},
        {"type": "tool_use", "id": "toolu_1", "name": "list_layers", "input": {}}
    ], "stop_reason": "tool_use"});
    let Step::Send(req) = a.receive(a.generation(), ok(reply), &mut s) else { panic!("expected a request") };
    let b = body(&req);
    let msgs = b["messages"].as_array().cloned().unwrap_or_default();
    assert_eq!(msgs.len(), 3);
    // The thinking block goes back unchanged; the result follows as a user message.
    assert_eq!(msgs[1]["content"][0]["signature"], "sig");
    assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
    assert_eq!(msgs[2]["content"][0]["tool_use_id"], "toolu_1");
    // The next user message joins the results' user message.
    let mut a2 = Agent::new(cfg("anthropic"), &s);
    let Step::Send(_) = a2.ask("x", &s) else { panic!("expected a request") };
    let _ =
        a2.receive(a2.generation(), ok(json!({"content": [{"type": "tool_use", "id": "t", "name": "list_layers", "input": {}}]})), &mut s);
    a2.stop();
    let Step::Send(req) = a2.ask("y", &s) else { panic!("expected a request") };
    let msgs = body(&req)["messages"].as_array().cloned().unwrap_or_default();
    assert!(msgs.windows(2).all(|w| w[0]["role"] != w[1]["role"]), "{msgs:?}");
    // A refusal runs nothing.
    let mut a3 = Agent::new(cfg("anthropic"), &s);
    let Step::Send(_) = a3.ask("x", &s) else { panic!("expected a request") };
    let refused = json!({"content": [{"type": "tool_use", "id": "t", "name": "run_command", "input": {"command": "layer.remove", "params": {"layer": "Cities"}}}], "stop_reason": "refusal"});
    assert_eq!(a3.receive(a3.generation(), ok(refused), &mut s), Step::Done);
    assert!(s.map().layer_by_name("Cities").is_some());
}

#[test]
fn gemini_format() {
    let mut s = sample();
    let mut a = Agent::new(cfg("gemini"), &s);
    let Step::Send(req) = a.ask("hide the rivers", &s) else { panic!("expected a request") };
    assert!(req.url.ends_with("/models/gemini-3.8-flash:generateContent"), "{}", req.url);
    let b = body(&req);
    let decls = b["tools"][0]["functionDeclarations"].as_array().cloned().unwrap_or_default();
    let run = decls.iter().find(|d| d["name"] == "run_command").cloned().unwrap_or(Value::Null);
    // A free-form object parameter goes as JSON text.
    assert_eq!(run["parameters"]["properties"]["params"]["type"], "string");
    let reply = json!({"candidates": [{"content": {"role": "model", "parts": [
        {"functionCall": {"name": "run_command", "args": {"command": "layer.visibility", "params": "{\"layer\": \"Rivers\", \"visible\": false}"}}, "thoughtSignature": "abc"}
    ]}, "finishReason": "STOP"}]});
    let Step::Send(req) = a.receive(a.generation(), ok(reply), &mut s) else { panic!("expected a request") };
    assert!(s.map().layer_by_name("Rivers").is_some_and(|l| !l.visible));
    let b = body(&req);
    let contents = b["contents"].as_array().cloned().unwrap_or_default();
    assert_eq!(contents[1]["parts"][0]["thoughtSignature"], "abc");
    assert_eq!(contents[2]["parts"][0]["functionResponse"]["name"], "run_command");
    assert!(contents[2]["parts"][0]["functionResponse"].get("id").is_none());
}

#[test]
fn steps_are_bounded_and_stop_works() {
    let mut s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let mut n = 0;
    run_blocking(
        &mut a,
        "loop forever",
        &mut s,
        |_| {
            n += 1;
            ok(call(&format!("c{n}"), "list_layers", json!({})))
        },
        |_, _, _| true,
    );
    assert_eq!(n, MAX_STEPS as usize);
    assert!(matches!(a.transcript.last(), Some(Entry::Note(_))));
    // A stale response (after a stop) is dropped.
    let Step::Send(_) = a.ask("again", &s) else { panic!("expected a request") };
    let g = a.generation();
    a.stop();
    assert_eq!(a.receive(g, ok(say("late")), &mut s), Step::Done);
    assert!(!a.transcript.iter().any(|e| matches!(e, Entry::Assistant(t) if t == "late")));
}

#[test]
fn missing_settings_are_reported() {
    let s = sample();
    let mut c = cfg("openai");
    c.key.clear();
    let mut a = Agent::new(c, &s);
    assert_eq!(a.ask("hi", &s), Step::Done);
    assert!(matches!(a.transcript.last(), Some(Entry::Error(_))));
    let mut c = cfg("ollama");
    c.key.clear();
    assert!(c.missing().is_none());
}

#[test]
fn model_lists() {
    let r = |v: Value| HttpResponse { status: 200, body: v.to_string().into_bytes(), request_id: None };
    assert_eq!(parse_models(Api::OpenAi, &r(json!({"data": [{"id": "b"}, {"id": "a"}]}))), Ok(vec!["a".to_string(), "b".to_string()]));
    let g = json!({"models": [
        {"name": "models/gemini-x", "supportedGenerationMethods": ["generateContent"]},
        {"name": "models/embed", "supportedGenerationMethods": ["embedContent"]}
    ]});
    assert_eq!(parse_models(Api::Gemini, &r(g)), Ok(vec!["gemini-x".to_string()]));
    let req = models_request(&cfg("anthropic"));
    assert_eq!(req.method, "GET");
    assert!(req.url.starts_with("https://api.anthropic.com/v1/models"));
}

#[test]
fn empty_replies_are_reported() {
    let mut s = sample();
    let mut a = Agent::new(cfg("ollama"), &s);
    let Step::Send(_) = a.ask("hi", &s) else { panic!("expected a request") };
    let empty = json!({"choices": [{"message": {"role": "assistant", "content": "", "reasoning": "…"}, "finish_reason": "stop"}]});
    assert_eq!(a.receive(a.generation(), ok(empty), &mut s), Step::Done);
    assert!(matches!(a.transcript.last(), Some(Entry::Note(n)) if n.contains("empty")));
}

#[test]
fn ordering_mixed_columns_never_panics() {
    use rustgis_engine::data::Value as V;
    let mut s = sample();
    let id = s.layer_id(&json!({"layer": "Cities"})).unwrap_or(0);
    // A column of mixed numbers, numeric text, words, NaN and nulls.
    let mixed = [V::Int(5), V::Text("abc".into()), V::Text("10".into()), V::Float(f64::NAN), V::Null, V::Text("9".into()), V::Bool(true)];
    if let Some(l) = s.map_mut().layer_mut(id) {
        let fi = l.dataset.fields.iter().position(|f| f.name == "name").unwrap_or(0);
        for (i, f) in l.dataset.features.iter_mut().enumerate() {
            if let Some(a) = f.attrs.get_mut(fi) {
                *a = mixed[i % mixed.len()].clone();
            }
        }
    }
    let t = ToolCall {
        id: "x".into(),
        name: "read_features".into(),
        args: json!({"layer": "Cities", "order_by": "name DESC", "fields": ["name"], "limit": 30}),
    };
    let r = tools::run(&mut s, &t);
    let v: Value = r.as_deref().ok().and_then(|r| serde_json::from_str(r).ok()).unwrap_or(Value::Null);
    let rows = v["rows"].as_array().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 30, "{r:?}");
    // Numbers (numeric text too) first, largest first; empty values last.
    assert_eq!(rows[0]["name"], "10", "{r:?}");
    assert!(rows.iter().all(|r| !r["name"].is_null()), "{r:?}");
}

#[test]
fn always_allow_is_per_command_and_never_for_reserved_actions() {
    let mut s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(_) = a.ask("x", &s) else { panic!("expected a request") };
    let dir = std::env::temp_dir();
    let export = |n: u32| json!({"command": "share.exportLayer", "params": {"layer": "Lakes", "path": dir.join(format!("rustgis-assistant-test-{n}.geojson"))}});
    let reply = json!({"choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [
        {"id": "c0", "type": "function", "function": {"name": "run_command", "arguments": export(1).to_string()}},
        {"id": "c1", "type": "function", "function": {"name": "run_command", "arguments": export(1).to_string()}},
        {"id": "c2", "type": "function", "function": {"name": "run_command", "arguments": export(2).to_string()}},
        {"id": "c3", "type": "function", "function": {"name": "run_command", "arguments": json!({"command": "map.addTiles", "params": {"url": "https://t.example/{z}/{x}/{y}.png"}}).to_string()}},
        {"id": "c4", "type": "function", "function": {"name": "run_command", "arguments": json!({"command": "project.new"}).to_string()}}
    ]}, "finish_reason": "tool_calls"}]});
    assert_eq!(a.receive(a.generation(), ok(reply), &mut s), Step::Confirm);
    assert!(a.pending(&s).is_some_and(|p| p.2));
    // Allowing the export for the conversation allows that file only: the next export (another
    // file) asks again, and so do the tiles.
    assert_eq!(a.decide(true, true, &mut s), Step::Confirm);
    assert!(a.pending(&s).is_some_and(|p| p.0.starts_with("share.exportLayer") && p.2));
    assert_eq!(a.decide(false, false, &mut s), Step::Confirm);
    assert!(a.pending(&s).is_some_and(|p| p.0.starts_with("map.addTiles")));
    assert_eq!(a.decide(false, false, &mut s), Step::Confirm);
    // Replacing the project is never allowed for good.
    assert!(a.pending(&s).is_some_and(|p| p.0.starts_with("project.new") && !p.2));
    let _ = a.decide(false, false, &mut s);
    for n in [1, 2] {
        let _ = std::fs::remove_file(dir.join(format!("rustgis-assistant-test-{n}.geojson")));
    }
}

#[test]
fn stopping_a_request_drops_its_message() {
    let s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(_) = a.ask("first", &s) else { panic!("expected a request") };
    a.stop();
    let Step::Send(req) = a.ask("second", &s) else { panic!("expected a request") };
    let msgs = body(&req)["messages"].as_array().cloned().unwrap_or_default();
    let users: Vec<&Value> = msgs.iter().filter(|m| m["role"] == "user").collect();
    assert_eq!(users.len(), 1);
    assert!(users[0]["content"].as_str().is_some_and(|t| t.starts_with("<map>") && t.ends_with("second")));
}

#[test]
fn malformed_params_are_refused() {
    let mut s = sample();
    let n = s.map().layers.len();
    for params in [json!("{\"layer\": "), json!("[1]"), json!(5)] {
        let c = ToolCall { id: "x".into(), name: "run_command".into(), args: json!({"command": "layer.remove", "params": params}) };
        assert!(tools::run(&mut s, &c).is_err());
    }
    assert_eq!(s.map().layers.len(), n, "no layer may be removed by a call whose parameters didn't parse");
    let c = ToolCall { id: "x".into(), name: "run_command".into(), args: json!({"command": "map.zoomFull", "params": "{}"}) };
    assert!(tools::run(&mut s, &c).is_ok());
}

#[test]
fn stopping_a_follow_up_keeps_the_history_valid() {
    let mut s = sample();
    for provider in ["anthropic", "openai", "gemini"] {
        let mut a = Agent::new(cfg(provider), &s);
        let Step::Send(_) = a.ask("x", &s) else { panic!("expected a request") };
        let reply = match provider {
            "anthropic" => json!({"content": [{"type": "tool_use", "id": "t", "name": "list_layers", "input": {}}]}),
            "gemini" => {
                json!({"candidates": [{"content": {"role": "model", "parts": [{"functionCall": {"name": "list_layers", "args": {}}}]}}]})
            }
            _ => call("t", "list_layers", json!({})),
        };
        // The calls ran and the follow-up request is out when the user stops.
        let Step::Send(_) = a.receive(a.generation(), ok(reply), &mut s) else { panic!("expected a follow-up") };
        a.stop();
        let Step::Send(req) = a.ask("y", &s) else { panic!("expected a request") };
        let b = body(&req);
        match provider {
            // Results and the new text share one user message.
            "anthropic" => {
                let msgs = b["messages"].as_array().cloned().unwrap_or_default();
                assert!(msgs.windows(2).all(|w| w[0]["role"] != w[1]["role"]), "{msgs:?}");
                let last = msgs.last().cloned().unwrap_or(Value::Null);
                assert_eq!(last["content"][0]["type"], "tool_result");
                assert_eq!(last["content"][1]["type"], "text");
            }
            // Tool results, then the user's message.
            "openai" => {
                let msgs = b["messages"].as_array().cloned().unwrap_or_default();
                let roles: Vec<&str> = msgs.iter().filter_map(|m| m["role"].as_str()).collect();
                assert_eq!(roles, ["system", "user", "assistant", "tool", "user"]);
            }
            _ => {
                let c = b["contents"].as_array().cloned().unwrap_or_default();
                assert_eq!(c.len(), 4);
                assert!(c[2]["parts"][0].get("functionResponse").is_some());
            }
        }
    }
}

// The MCP server.

use crate::{docs, mcp};

/// Sends one request and gives back its `result` (or the whole reply when it is an error).
fn rpc(server: &mcp::Server, s: &mut Session, method: &str, params: Value) -> Value {
    let msg = json!({"jsonrpc": "2.0", "id": 7, "method": method, "params": params}).to_string();
    let reply: Value = serde_json::from_str(&server.handle(s, &msg).unwrap()).unwrap();
    assert_eq!(reply["id"], 7);
    if reply.get("error").is_some() { reply } else { reply["result"].clone() }
}

fn call_tool(server: &mcp::Server, s: &mut Session, name: &str, args: Value) -> (bool, String) {
    let r = rpc(server, s, "tools/call", json!({"name": name, "arguments": args}));
    (r["isError"] == true, r["content"][0]["text"].as_str().unwrap_or("").to_string())
}

#[test]
fn mcp_initializes_and_lists_tools() {
    let (server, mut s) = (mcp::Server::default(), sample());
    let r = rpc(&server, &mut s, "initialize", json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t"}}));
    assert_eq!(r["protocolVersion"], "2025-03-26");
    assert_eq!(r["serverInfo"]["name"], "rustgis");
    assert!(r["capabilities"]["tools"].is_object());
    // A version we don't know gets our newest.
    let r = rpc(&server, &mut s, "initialize", json!({"protocolVersion": "1999-01-01"}));
    assert_eq!(r["protocolVersion"], mcp::VERSIONS[0]);
    // Notifications get no reply.
    assert!(server.handle(&mut s, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    assert_eq!(rpc(&server, &mut s, "ping", Value::Null), json!({}));

    let tools = rpc(&server, &mut s, "tools/list", json!({}));
    let names: Vec<&str> = tools["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for want in [
        "list_layers",
        "search_commands",
        "run_command",
        "search_tools",
        "describe_tool",
        "run_tool",
        "read_features",
        "view_map",
        "search_docs",
        "read_doc",
    ] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }
    for t in tools["tools"].as_array().unwrap() {
        assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
        assert!(t["description"].as_str().is_some_and(|d| !d.is_empty()));
        let runs = matches!(t["name"].as_str(), Some("run_command" | "run_tool"));
        assert_eq!(t["annotations"]["readOnlyHint"], !runs, "{}", t["name"]);
        assert_eq!(t["annotations"]["destructiveHint"], runs, "{}", t["name"]);
    }
}

#[test]
fn mcp_protocol_errors() {
    let (server, mut s) = (mcp::Server::default(), sample());
    let r: Value = serde_json::from_str(&server.handle(&mut s, "{not json").unwrap()).unwrap();
    assert_eq!(r["error"]["code"], -32700);
    assert_eq!(rpc(&server, &mut s, "nope/nothing", json!({}))["error"]["code"], -32601);
    assert_eq!(rpc(&server, &mut s, "tools/call", json!({"name": "no_such_tool"}))["error"]["code"], -32602);
    assert_eq!(rpc(&server, &mut s, "tools/call", json!({"name": "list_layers", "arguments": [1]}))["error"]["code"], -32602);
    assert_eq!(rpc(&server, &mut s, "resources/read", json!({"uri": "rustgis://docs/nope.md"}))["error"]["code"], -32002);
    // Batches: one reply per request, none for notifications only.
    let batch = r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"},{"jsonrpc":"2.0","id":2,"method":"nope"}]"#;
    let r: Value = serde_json::from_str(&server.handle(&mut s, batch).unwrap()).unwrap();
    assert_eq!(r.as_array().map(Vec::len), Some(2));
    assert_eq!(r[0]["id"], 1);
    assert_eq!(r[1]["error"]["code"], -32601);
    assert!(server.handle(&mut s, r#"[{"jsonrpc":"2.0","method":"notifications/initialized"}]"#).is_none());
    let r: Value = serde_json::from_str(&server.handle(&mut s, "[]").unwrap()).unwrap();
    assert_eq!(r["error"]["code"], -32600);
    let big = Value::Array(vec![json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}); mcp::MAX_BATCH + 1]).to_string();
    let r: Value = serde_json::from_str(&server.handle(&mut s, &big).unwrap()).unwrap();
    assert_eq!(r["error"]["code"], -32600);
    // A response to us (we send no requests) gets no reply.
    assert!(server.handle(&mut s, r#"{"jsonrpc":"2.0","id":1,"result":{}}"#).is_none());
}

/// `expects_reply` must answer exactly when the server answers: a bare non-object such as `5`
/// is an INVALID_REQUEST (`handle_value`), a notification is not.
#[test]
fn mcp_expects_reply_matches_the_server() {
    assert!(mcp::expects_reply("5"), "a bare non-object gets INVALID_REQUEST, so a reply is owed");
    assert!(mcp::expects_reply(r#""x""#));
    assert!(mcp::expects_reply(r#"[1]"#), "a batch element that is a non-object");
    assert!(mcp::expects_reply(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#));
    assert!(mcp::expects_reply("{not json"), "a parse error is answered");
    assert!(!mcp::expects_reply(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#));
    assert!(!mcp::expects_reply(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#));
    assert!(!mcp::expects_reply(r#"{"jsonrpc":"2.0","id":1,"error":{"code":1}}"#));
}

#[test]
fn mcp_runs_commands_and_shows_the_map() {
    let (server, mut s) = (mcp::Server::default(), sample());
    let (err, text) = call_tool(&server, &mut s, "list_layers", json!({}));
    assert!(!err && text.contains("Countries"), "{text}");
    let (err, text) = call_tool(&server, &mut s, "run_command", json!({"command": "map.basemap", "params": {"name": "ocean"}}));
    assert!(!err, "{text}");
    let (err, text) = call_tool(&server, &mut s, "read_features", json!({"layer": "Cities", "limit": 2}));
    assert!(!err && text.contains("\"rows\""), "{text}");

    let r = rpc(&server, &mut s, "tools/call", json!({"name": "view_map", "arguments": {"width": 320, "height": 200}}));
    assert_eq!(r["isError"], false);
    assert_eq!(r["content"][0]["type"], "image");
    assert_eq!(r["content"][0]["mimeType"], "image/png");
    use base64::Engine;
    let png = base64::engine::general_purpose::STANDARD.decode(r["content"][0]["data"].as_str().unwrap()).unwrap();
    assert!(png.starts_with(b"\x89PNG"));
    // Junk sizes are clamped, not refused.
    let r = rpc(&server, &mut s, "tools/call", json!({"name": "view_map", "arguments": {"width": -5, "height": 1e300}}));
    assert_eq!(r["isError"], false);
}

#[test]
fn mcp_limits_what_the_agent_may_do() {
    let path = std::env::temp_dir().join(format!("rustgis-mcp-{}.png", std::process::id()));
    let export = json!({"command": "share.exportMap", "params": {"path": path.to_string_lossy()}});

    let (server, mut s) = (mcp::Server::new(false), sample());
    let (err, text) = call_tool(&server, &mut s, "run_command", export.clone());
    assert!(err && text.contains("--yes"), "{text}");
    assert!(!path.exists());
    // The user's own automation never runs, even when allowed.
    let allowed = mcp::Server::new(true);
    for server in [&server, &allowed] {
        let (err, text) = call_tool(server, &mut s, "run_command", json!({"command": "script.run", "params": {"code": "x"}}));
        assert!(err && text.contains("reserved"), "{text}");
        let (err, _) = call_tool(server, &mut s, "run_command", json!({"command": "plugin.uninstall", "params": {"id": "x"}}));
        assert!(err);
    }
    // Opening a project or template reads a file.
    for (cmd, params) in [("project.open", json!({"path": "a.rgis"})), ("project.fromTemplate", json!({"path": "a.rgistemplate"}))] {
        let (err, text) = call_tool(&server, &mut s, "run_command", json!({"command": cmd, "params": params}));
        assert!(err && text.contains("--yes"), "{cmd}: {text}");
    }
    // A project from text can bring web layers in, which drawing the map would fetch.
    let (err, text) = call_tool(&server, &mut s, "run_command", json!({"command": "project.openText", "params": {"text": "{}"}}));
    assert!(err && text.contains("--yes"), "{text}");
    // The project is the agent's own: replacing it and undoing are allowed.
    let (err, text) = call_tool(&server, &mut s, "run_command", json!({"command": "project.new"}));
    assert!(!err, "{text}");
    let (err, text) = call_tool(&server, &mut s, "run_command", json!({"command": "project.sample"}));
    assert!(!err, "{text}");

    let (err, text) = call_tool(&allowed, &mut s, "run_command", export);
    assert!(!err, "{text}");
    assert!(path.exists());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn mcp_serves_the_docs() {
    let (server, mut s) = (mcp::Server::default(), sample());
    let (err, text) = call_tool(&server, &mut s, "search_docs", json!({"query": "graduated colours"}));
    assert!(!err, "{text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    assert!(v["results"].as_array().unwrap().iter().any(|r| r["page"] == "user-guide/cartography.md"), "{text}");
    assert!(v["results"][0]["url"].as_str().unwrap().starts_with(docs::SITE));
    // A plural finds the singular; words ending in "ss" keep their ending.
    let (_, text) = call_tool(&server, &mut s, "search_docs", json!({"query": "classes"}));
    assert!(text.contains("class"), "{text}");
    let (err, _) = call_tool(&server, &mut s, "search_docs", json!({"query": "?"}));
    assert!(err);

    let (err, text) = call_tool(&server, &mut s, "read_doc", json!({"page": "cli"}));
    assert!(!err && text.contains("Command Line"), "{text}");
    let (err, text) = call_tool(&server, &mut s, "read_doc", json!({"page": "cli.md", "section": "chaining commands"}));
    assert!(!err && text.contains("--cmd") && text.contains("#chaining-commands"), "{text}");
    // A section comes with its heading and its subsections.
    let (err, text) = call_tool(&server, &mut s, "read_doc", json!({"page": "mcp.md", "section": "MCP Server"}));
    assert!(!err && text.contains("## Tools") && text.contains("# MCP Server"), "{text}");
    let (err, text) = call_tool(&server, &mut s, "read_doc", json!({"page": "cli.md", "section": "no such section"}));
    assert!(err && text.contains("Chaining commands"), "{text}");
    let (err, _) = call_tool(&server, &mut s, "read_doc", json!({"page": "../../etc/passwd"}));
    assert!(err);

    let list = rpc(&server, &mut s, "resources/list", json!({}));
    assert_eq!(list["resources"].as_array().unwrap().len(), docs::PAGES.len());
    let r = rpc(&server, &mut s, "resources/read", json!({"uri": "rustgis://docs/user-guide/maps.md"}));
    assert!(r["contents"][0]["text"].as_str().unwrap().starts_with("# "));
}

#[test]
fn docs_pages_and_addresses() {
    // Every page in docs/ is built in (add new pages to `docs::PAGES`).
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    let mut found = Vec::new();
    let mut dirs = vec![root.clone()];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "md") {
                found.push(p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    for f in &found {
        assert!(docs::page(f).is_some(), "docs/{f} is missing from docs::PAGES");
    }
    assert_eq!(found.len(), docs::PAGES.len());

    assert_eq!(docs::page("index.md").unwrap().url(), "https://rustgis.app/");
    assert_eq!(docs::page("user-guide").unwrap().url(), "https://rustgis.app/user-guide/");
    assert_eq!(docs::page("https://rustgis.app/user-guide/maps/").unwrap().path, "user-guide/maps.md");
    assert_eq!(docs::slug("Chaining commands"), "chaining-commands");
    assert_eq!(docs::slug("What the agent may do"), "what-the-agent-may-do");
    // Headings in code blocks aren't sections.
    let text = "# T\nintro\n```sh\n# a comment\n```\n~~~\n# also code\n~~~\n    # indented code\n````md\n```\n# inside a longer fence\n```not a close\n````\n## Next\nbody\n### Sub\nmore\n## Last\n#tag\n";
    let secs = docs::sections(text);
    assert_eq!(secs.iter().map(|s| s.0).collect::<Vec<_>>(), ["T", "Next", "Sub", "Last"]);
    assert_eq!(secs.get(1).map(|s| s.1), Some("body\n"));
}

#[test]
fn mcp_hostile_messages_never_panic() {
    let junk = [
        Value::Null,
        json!(1),
        json!("x"),
        json!([]),
        json!({}),
        json!({"name": 5}),
        json!({"name": "run_command", "arguments": {"command": 5, "params": "{"}}),
        json!({"name": "run_tool", "arguments": {"tool": "buffer", "params": {"input": [], "distance": "far"}}}),
        json!({"name": "read_features", "arguments": {"layer": {}, "limit": -1, "order_by": 3, "fields": [1]}}),
        json!({"name": "view_map", "arguments": {"width": "big"}}),
        json!({"name": "search_docs", "arguments": {"query": "é".repeat(5000), "limit": 1e30}}),
        json!({"name": "read_doc", "arguments": {"page": "", "section": "#"}}),
        json!({"uri": 3}),
        json!({"protocolVersion": []}),
    ];
    let methods = [
        "initialize",
        "ping",
        "tools/list",
        "tools/call",
        "resources/list",
        "resources/read",
        "resources/templates/list",
        "prompts/list",
        "x",
    ];
    for allow in [false, true] {
        let (server, mut s) = (mcp::Server::new(allow), sample());
        for m in methods {
            for p in &junk {
                let msg = json!({"jsonrpc": "2.0", "id": "a", "method": m, "params": p}).to_string();
                assert!(server.handle(&mut s, &msg).is_some(), "{m} {p}");
            }
        }
        for raw in ["", "[]", "[{}]", "null", "\"x\"", r#"{"id":{},"method":"ping"}"#, r#"{"id":1,"method":7}"#, "\u{0}"] {
            let _ = server.handle(&mut s, raw);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Signing in with ChatGPT: the Responses wire format and the credentials

/// A JWT with these claims (unsigned: the tests only read it).
fn jwt(claims: Value) -> String {
    use base64::Engine;
    let b64 = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.as_bytes());
    format!("{}.{}.{}", b64(r#"{"alg":"none"}"#), b64(&claims.to_string()), b64("sig"))
}

fn chatgpt_credentials(expires_at: i64) -> chatgpt::Credentials {
    chatgpt::Credentials {
        client_id: "oaiapp_synthetic".into(),
        subject: "user-1".into(),
        email: "ann@example.com".into(),
        id_token: jwt(json!({"sub": "user-1"})),
        access_token: jwt(json!({"exp": expires_at})),
        refresh_token: "r1".into(),
        expires_at,
        scopes: chatgpt::SCOPES.split(' ').map(str::to_string).collect(),
        plan_declined: false,
        saved_at: "2026-10-14T00:00:00Z".into(),
        session: "session-1".into(),
    }
}

fn chatgpt_cfg() -> Config {
    let mut c = cfg("chatgpt");
    c.sign_in(&chatgpt_credentials(4_000_000_000));
    c
}

/// A streamed reply, as the API sends it: the items finished, then the final response.
fn sse(items: &[Value], status: &str) -> Result<HttpResponse, String> {
    let mut s = String::from("event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n");
    for (i, item) in items.iter().enumerate() {
        s.push_str(&format!(
            "event: response.output_item.done\ndata: {}\n\n",
            json!({"type": "response.output_item.done", "output_index": i, "item": item})
        ));
    }
    let done = json!({"type": format!("response.{status}"), "response": {"id": "resp_1", "status": status, "output": items,
        "incomplete_details": if status == "incomplete" { json!({"reason": "max_output_tokens"}) } else { Value::Null }}});
    s.push_str(&format!("event: response.{status}\ndata: {done}\n\n"));
    Ok(HttpResponse { status: 200, body: s.into_bytes(), request_id: None })
}

#[test]
fn responses_request_shape() {
    let s = sample();
    let mut a = Agent::new(chatgpt_cfg(), &s);
    let Step::Send(req) = a.ask("Zoom to Paris", &s) else { panic!("expected a request") };
    assert_eq!(req.url, "https://api.openai.com/v1/responses");
    let h = |k: &str| req.headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    assert!(h("Authorization").is_some_and(|v| v.starts_with("Bearer ey")));
    assert_eq!(req.headers.iter().filter(|(k, _)| !["Content-Type", "Accept", "Authorization"].contains(&k.as_str())).count(), 0);
    let b = body(&req);
    assert_eq!(b["model"], "gpt-6.1-sol");
    // What a ChatGPT plan requires, and leaves out.
    assert_eq!(b["stream"], true);
    assert_eq!(b["store"], false);
    for unsupported in ["max_output_tokens", "temperature", "top_p", "metadata", "user", "previous_response_id", "truncation"] {
        assert!(b.get(unsupported).is_none(), "{unsupported}");
    }
    assert_eq!(b["include"][0], "reasoning.encrypted_content");
    assert!(b["instructions"].as_str().is_some_and(|i| !i.is_empty()));
    let input = b["input"].as_array().cloned().unwrap_or_default();
    assert!(!input.iter().any(|i| i["role"] == "system"), "system messages are refused");
    assert_eq!(input[0]["role"], "user");
    assert!(input[0]["content"][0]["text"].as_str().is_some_and(|t| t.ends_with("Zoom to Paris")));
    // The tools are grouped in a namespace.
    let tools = b["tools"].as_array().cloned().unwrap_or_default();
    assert_eq!(tools.len(), 1);
    assert_eq!((tools[0]["type"].as_str(), tools[0]["name"].as_str()), (Some("namespace"), Some(TOOL_NAMESPACE)));
    let functions = tools[0]["tools"].as_array().cloned().unwrap_or_default();
    assert!(functions.iter().any(|t| t["type"] == "function" && t["name"] == "run_command" && t["parameters"]["type"] == "object"));
}

#[test]
fn responses_round_trip_replays_reasoning_and_calls() {
    let mut s = sample();
    let mut a = Agent::new(chatgpt_cfg(), &s);
    let call = json!({"id": "fc_1", "type": "function_call", "status": "completed", "call_id": "call_1", "name": "run_command",
        "arguments": json!({"command": "map.goToXY", "params": {"lon": 2.35, "lat": 48.85, "scale": 5000000}}).to_string()});
    let reasoning = json!({"id": "rs_1", "type": "reasoning", "summary": [], "encrypted_content": "gAAAA-secret"});
    let answer = json!({"id": "msg_1", "type": "message", "role": "assistant", "status": "completed",
        "content": [{"type": "output_text", "text": "Done: **Paris**.", "annotations": []}]});
    let mut replies: VecDeque<Result<HttpResponse, String>> =
        VecDeque::from(vec![sse(&[reasoning.clone(), call.clone()], "completed"), sse(&[answer], "completed")]);
    let mut requests = Vec::new();
    run_blocking(
        &mut a,
        "Zoom to Paris",
        &mut s,
        |req| {
            requests.push(body(req));
            replies.pop_front().unwrap_or_else(|| Err("no more replies".into()))
        },
        |_, _, _| true,
    );
    let c = s.map().center_lonlat();
    assert!((c.x - 2.35).abs() < 0.01, "map moved to {c:?}");
    assert!(matches!(a.transcript.get(1), Some(Entry::Tool { error: false, .. })), "{:?}", a.transcript);
    assert!(matches!(a.transcript.last(), Some(Entry::Assistant(t)) if t.contains("Paris")));
    // The second request replays the reasoning (encrypted, without the server's id) and the call,
    // then gives the result.
    let input = requests.get(1).and_then(|b| b["input"].as_array().cloned()).unwrap_or_default();
    let types: Vec<&str> = input.iter().filter_map(|i| i["type"].as_str()).collect();
    assert_eq!(types, ["message", "reasoning", "function_call", "function_call_output"], "{input:?}");
    assert_eq!(input[1]["encrypted_content"], "gAAAA-secret");
    assert!(input[1].get("id").is_none() && input[2].get("id").is_none());
    assert_eq!(input[2]["call_id"], "call_1");
    assert_eq!(input[3]["call_id"], "call_1");
    assert!(input[3]["output"].as_str().is_some_and(|o| o.contains("ok")));
}

#[test]
fn responses_reply_edge_cases() {
    // Cut short.
    let msg = json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Half"}]});
    let r = parse_reply(Api::Responses, &sse(&[msg], "incomplete").unwrap()).unwrap();
    assert_eq!(r.text, "Half");
    assert!(r.cut.as_deref().is_some_and(|c| c.contains("length")));
    // A refusal runs no calls.
    let refusal = json!({"type": "message", "role": "assistant", "content": [{"type": "refusal", "refusal": "no"}]});
    let call = json!({"type": "function_call", "call_id": "c", "name": "run_command", "arguments": "{}"});
    let r = parse_reply(Api::Responses, &sse(&[refusal, call], "completed").unwrap()).unwrap();
    assert!(r.calls.is_empty() && r.cut.as_deref().is_some_and(|c| c.contains("declined")));
    // A failed response, and an error event.
    let body = "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"message\":\"overloaded\"}}}\n\n";
    let e = parse_reply(Api::Responses, &HttpResponse { status: 200, body: body.into(), request_id: None }).unwrap_err();
    assert!(e.to_string().contains("overloaded"), "{e}");
    let body = "data: {\"type\":\"error\",\"error\":{\"message\":\"bad token\"}}\n\n";
    assert!(
        parse_reply(Api::Responses, &HttpResponse { status: 200, body: body.into(), request_id: None })
            .unwrap_err()
            .to_string()
            .contains("bad token")
    );
    // An unfinished stream, or one whose items arrived without `response.completed`.
    let body = "data: {\"type\":\"response.created\",\"response\":{}}\n\n";
    assert!(
        parse_reply(Api::Responses, &HttpResponse { status: 200, body: body.into(), request_id: None })
            .unwrap_err()
            .to_string()
            .contains("ended")
    );
    let mut cut = String::from_utf8(sse(&[json!({"type": "message", "content": []})], "completed").unwrap().body).unwrap();
    cut.truncate(cut.find("event: response.completed").unwrap_or(cut.len()));
    assert!(
        parse_reply(Api::Responses, &HttpResponse { status: 200, body: cut.into_bytes(), request_id: None })
            .unwrap_err()
            .to_string()
            .contains("ended")
    );
    // The admission check's `detail`, and bodies that aren't JSON.
    let e = parse_reply(
        Api::Responses,
        &HttpResponse { status: 503, body: br#"{"detail":"Direct routing is unavailable"}"#.to_vec(), request_id: None },
    );
    assert!(e.unwrap_err().to_string().contains("unavailable"));
    let e = parse_reply(Api::Responses, &HttpResponse { status: 401, body: b"nope".to_vec(), request_id: None }).unwrap_err();
    assert!(e.to_string().starts_with("HTTP 401"), "{e}");
    // A plain JSON reply (no stream) reads the same.
    let v = json!({"status": "completed", "output": [{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Hi"}]}]});
    let r = parse_reply(Api::Responses, &HttpResponse { status: 200, body: v.to_string().into_bytes(), request_id: None }).unwrap();
    assert_eq!(r.text, "Hi");
    // Unparsable arguments are kept; a namespaced name is the tool's; a call without an id gets
    // one that its replay keeps.
    let call = json!({"type": "function_call", "call_id": "c", "name": "run_command", "arguments": "{not json"});
    let r = parse_reply(Api::Responses, &sse(&[call], "completed").unwrap()).unwrap();
    assert_eq!(r.calls[0].args["_unparsed"], "{not json");
    let call = json!({"type": "function_call", "name": format!("{TOOL_NAMESPACE}.run_command"), "arguments": "{}"});
    let r = parse_reply(Api::Responses, &sse(&[call], "completed").unwrap()).unwrap();
    assert_eq!(r.calls[0].name, "run_command");
    assert_eq!(r.native[0]["call_id"].as_str(), Some(r.calls[0].id.as_str()));
}

#[test]
fn responses_plan_errors_say_what_to_do() {
    let err = |status: u16, code: &str| {
        let body = json!({"error": {"code": code, "message": "synthetic", "param": "tools[0]"}}).to_string().into_bytes();
        parse_reply(Api::Responses, &HttpResponse { status, body, request_id: None }).unwrap_err().to_string()
    };
    assert!(err(429, "subscription_sharing_usage_limit_exceeded").contains(chatgpt::USAGE_URL));
    assert!(err(503, "subscription_sharing_usage_unavailable").contains("try again"));
    assert!(err(403, "subscription_sharing_user_not_eligible").contains("unavailable"));
    assert!(err(400, "subscription_sharing_unsupported_capability").contains("tools[0]"));
    assert!(err(401, "subscription_sharing_invalid_user").contains("configuration"));
    assert_eq!(err(400, "something_else"), "HTTP 400: synthetic [something_else] [tools[0]]");
    // After the stream has begun, as `response.failed`.
    let failed = json!({"type": "response.failed", "response": {"status": "failed", "error": {"code": "subscription_sharing_usage_limit_exceeded"}}});
    let body = format!("data: {failed}\n\n");
    let e = parse_reply(Api::Responses, &HttpResponse { status: 200, body: body.into_bytes(), request_id: None }).unwrap_err();
    assert!(e.to_string().contains(chatgpt::USAGE_URL), "{e}");
}

#[test]
fn streamed_replies_may_be_larger() {
    let s = sample();
    let Step::Send(stream) = Agent::new(chatgpt_cfg(), &s).ask("Zoom to Paris", &s) else { panic!("expected a request") };
    assert_eq!(reply_limit(&stream), MAX_STREAM);
    let Step::Send(plain) = Agent::new(cfg("openai"), &s).ask("Zoom to Paris", &s) else { panic!("expected a request") };
    assert_eq!(reply_limit(&plain), MAX_RESPONSE);
}

#[test]
fn responses_models_from_the_catalog() {
    let cfg = chatgpt_cfg();
    let req = models_request(&cfg);
    assert_eq!(req.url, "https://api.openai.com/v1/models");
    assert!(req.headers.iter().any(|(k, v)| k == "Authorization" && v.starts_with("Bearer ")));
    let v = json!({"models": [{"slug": "gpt-6.1-sol", "visibility": "list"}, {"slug": "gpt-reserve", "visibility": "hide"}, {"slug": "gpt-5.6-sol", "visibility": "list"}]});
    let m = parse_models(Api::Responses, &HttpResponse { status: 200, body: v.to_string().into_bytes(), request_id: None }).unwrap();
    assert_eq!(m, ["gpt-6.1-sol", "gpt-5.6-sol"], "hidden models are left out, the order is the catalog's");
    assert!(cfg.missing().is_none());
    let mut out = cfg;
    out.key.clear();
    assert_eq!(out.missing(), Some("Sign in with your ChatGPT account."));
}

#[test]
fn chatgpt_credentials_file_shape() {
    let c = chatgpt_credentials(1_800_000_000);
    assert!(c.signed_in() && c.plan_allowed());
    assert_eq!(chatgpt::Credentials::from_json(&c.to_json()).unwrap(), c);
    assert_eq!(c.to_json()["issuer"], chatgpt::ISSUER);
    assert!(!c.expires_soon(1_800_000_000 - 3600));
    assert!(c.expires_soon(1_800_000_000 - 60));
    assert!(chatgpt::Credentials { expires_at: 0, ..c.clone() }.expires_soon(0), "an unknown expiry means renew");
    // A sign-out keeps the registration only.
    let r = c.registration();
    assert!(!r.signed_in() && r.refresh_token.is_empty() && r.id_token.is_empty());
    assert_eq!((r.client_id.as_str(), r.subject.as_str(), r.email.as_str()), ("oaiapp_synthetic", "user-1", "ann@example.com"));
    assert_eq!(chatgpt::Credentials::from_json(&r.to_json()).unwrap(), r);
    // Without a registration, or with the registration entrypoint's id, there is none.
    assert!(chatgpt::Credentials::from_json(&json!({"access_token": "a"})).is_err());
    assert!(chatgpt::Credentials::from_json(&json!({"client_id": chatgpt::REGISTER})).is_err());
    // Without the plan's scope, nothing is sent.
    let mut no_plan = c.clone();
    no_plan.scopes.retain(|s| s != chatgpt::PLAN_SCOPE);
    assert!(!no_plan.plan_allowed());
    // Never in Debug output.
    let dbg = format!("{c:?}");
    assert!(!dbg.contains("ey") && !dbg.contains("r1"), "{dbg}");
    let cfg = chatgpt_cfg();
    assert!(!format!("{cfg:?}").contains(&cfg.key));
    // Credentials replace any on a request.
    let req = c.authorize(HttpRequest {
        method: "GET",
        url: String::new(),
        headers: vec![("authorization".into(), "Bearer old".into())],
        body: None,
    });
    assert_eq!(req.headers, [("Authorization".to_string(), format!("Bearer {}", c.access_token))]);
}

#[test]
fn chatgpt_host_id() {
    let id = chatgpt::host_id_from([0xff; 16]);
    assert_eq!(id, "urn:uuid:ffffffff-ffff-4fff-bfff-ffffffffffff");
    assert!(chatgpt::valid_host_id(&id));
    assert!(chatgpt::valid_host_id(&chatgpt::host_id_from([0; 16])));
    for bad in [
        "",
        "urn:uuid:",
        "ffffffff-ffff-4fff-bfff-ffffffffffff",
        "urn:uuid:ffffffff-ffff-4fff-bfff-fffffffffff",
        "urn:uuid:gfffffff-ffff-4fff-bfff-ffffffffffff",
    ] {
        assert!(!chatgpt::valid_host_id(bad), "{bad}");
    }
}

#[test]
fn chatgpt_authorization_page() {
    let challenge = chatgpt::pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
    assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM", "RFC 7636's example");
    let mut a = chatgpt::Authorization {
        client_id: chatgpt::REGISTER,
        host_id: "urn:uuid:00000000-0000-4000-8000-000000000000",
        redirect_uri: "http://127.0.0.1:1455/auth/callback",
        challenge: &challenge,
        state: "st ate",
        nonce: "n1",
        login_hint: "ann@example.com",
        id_token_hint: "id.token.hint",
        consent: false,
    };
    let params = |url: &str| chatgpt::query_params(url.split_once('?').map_or("", |(_, q)| q));
    let get = |p: &[(String, String)], k: &str| p.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    // A first sign-in registers RustGIS, without hints.
    let url = chatgpt::authorize_url(&a);
    assert!(url.starts_with("https://auth.openai.com/api/accounts/authorize?"));
    let p = params(&url);
    for (k, v) in [
        ("response_type", "code"),
        ("client_id", "dynamic_agent_client"),
        ("agent_name_hint", "RustGIS"),
        ("ext_agent_host_id", "urn:uuid:00000000-0000-4000-8000-000000000000"),
        ("redirect_uri", "http://127.0.0.1:1455/auth/callback"),
        ("scope", "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct"),
        ("resource", "https://api.openai.com/v1"),
        ("state", "st ate"),
        ("nonce", "n1"),
        ("code_challenge_method", "S256"),
    ] {
        assert_eq!(get(&p, k).as_deref(), Some(v), "{k}");
    }
    assert!(get(&p, "login_hint").is_none() && get(&p, "id_token_hint").is_none() && get(&p, "prompt").is_none());
    assert!(url.contains("state=st%20ate") && !url.contains(' '));
    // Later sign-ins use the issued client id and the hints, without the name.
    a.client_id = "oaiapp_synthetic";
    a.consent = true;
    let p = params(&chatgpt::authorize_url(&a));
    assert_eq!(get(&p, "client_id").as_deref(), Some("oaiapp_synthetic"));
    assert!(get(&p, "agent_name_hint").is_none());
    assert_eq!(get(&p, "login_hint").as_deref(), Some("ann@example.com"));
    assert_eq!(get(&p, "id_token_hint").as_deref(), Some("id.token.hint"), "skips choosing the account again");
    a.id_token_hint = "";
    assert!(get(&params(&chatgpt::authorize_url(&a)), "id_token_hint").is_none(), "the shareable page leaves it out");
    a.id_token_hint = "id.token.hint";
    let p = params(&chatgpt::authorize_url(&a));
    assert_eq!(get(&p, "prompt").as_deref(), Some("consent"), "asks again after the plan was declined");
}

#[test]
fn chatgpt_callback_client_id() {
    let q = |s: &str| chatgpt::query_params(s);
    assert_eq!(chatgpt::callback_client_id(&q("code=c&client_id=oaiapp_1"), chatgpt::REGISTER).as_deref(), Ok("oaiapp_1"));
    for bad in ["code=c", "code=c&client_id=", "code=c&client_id=dynamic_agent_client", "client_id=oaiapp_1&client_id=oaiapp_2"] {
        assert!(chatgpt::callback_client_id(&q(bad), chatgpt::REGISTER).is_err(), "{bad}");
    }
    assert_eq!(chatgpt::callback_client_id(&q("code=c"), "oaiapp_1").as_deref(), Ok("oaiapp_1"));
    assert_eq!(chatgpt::callback_client_id(&q("code=c&client_id=oaiapp_1"), "oaiapp_1").as_deref(), Ok("oaiapp_1"));
    assert!(chatgpt::callback_client_id(&q("code=c&client_id=oaiapp_2"), "oaiapp_1").is_err(), "never another registration");
}

#[test]
fn chatgpt_token_requests_and_replies() {
    let c = chatgpt_credentials(1_800_000_000);
    let form = |req: &HttpRequest| chatgpt::query_params(req.body.as_deref().unwrap_or(""));
    let req = chatgpt::exchange_request("oaiapp_synthetic", "c0de", "http://127.0.0.1:1455/auth/callback", "verifier");
    assert_eq!(req.url, "https://auth.openai.com/api/accounts/oauth/token");
    assert!(req.headers.iter().any(|(_, v)| v == "application/x-www-form-urlencoded"));
    assert_eq!(
        form(&req),
        [
            ("grant_type", "authorization_code"),
            ("client_id", "oaiapp_synthetic"),
            ("code", "c0de"),
            ("code_verifier", "verifier"),
            ("redirect_uri", "http://127.0.0.1:1455/auth/callback"),
            ("resource", "https://api.openai.com/v1"),
        ]
        .map(|(k, v)| (k.to_string(), v.to_string()))
    );
    let req = chatgpt::refresh_request(&c);
    assert_eq!(req.url, "https://auth.openai.com/api/accounts/oauth/token");
    assert_eq!(
        form(&req),
        [
            ("grant_type", "refresh_token"),
            ("client_id", "oaiapp_synthetic"),
            ("refresh_token", "r1"),
            ("resource", "https://api.openai.com/v1")
        ]
        .map(|(k, v)| (k.to_string(), v.to_string()))
    );
    let req = chatgpt::revoke_request("https://auth.openai.com/api/accounts/oauth/revoke", "oaiapp_synthetic", "r1");
    assert_eq!(
        form(&req),
        [("token", "r1"), ("token_type_hint", "refresh_token"), ("client_id", "oaiapp_synthetic")]
            .map(|(k, v)| (k.to_string(), v.to_string()))
    );
    // A grant: expiry from `expires_in`, the scopes granted.
    let resp = |v: Value| HttpResponse { status: 200, body: v.to_string().into_bytes(), request_id: None };
    let g = chatgpt::parse_grant(&resp(json!({"access_token": "a2", "refresh_token": "r2", "id_token": "i2", "expires_in": 3600,
        "scope": "openid email chatgpt.tokens.use.direct"})))
    .unwrap();
    let account = chatgpt::Account { email: "ann@example.com".into(), subject: "user-1".into() };
    let signed = chatgpt::signed_in("oaiapp_synthetic", &account, &g, 1_000);
    assert_eq!((signed.expires_at, signed.plan_allowed(), signed.plan_declined), (4_600, true, false));
    assert_eq!(signed.saved_at, "1970-01-01T00:16:40Z");
    let declined = chatgpt::parse_grant(&resp(json!({"access_token": "a", "scope": "openid email"}))).unwrap();
    let d = chatgpt::signed_in("oaiapp_synthetic", &account, &declined, 0);
    assert!(!d.plan_allowed() && d.plan_declined);
    // A renewal replaces the token set together; what isn't sent stays.
    let fresh = chatgpt::renewed(&c, &chatgpt::parse_grant(&resp(json!({"access_token": "a3", "expires_in": 60}))).unwrap(), 10);
    assert_eq!((fresh.access_token.as_str(), fresh.refresh_token.as_str(), fresh.expires_at), ("a3", "r1", 70));
    assert_eq!((fresh.id_token.as_str(), fresh.scopes.clone()), (c.id_token.as_str(), c.scopes.clone()));
    // A renewal that drops the plan's scope asks for it again at the next sign-in.
    let dropped = chatgpt::renewed(&c, &chatgpt::parse_grant(&resp(json!({"access_token": "a4", "scope": "openid email"}))).unwrap(), 0);
    assert!(!dropped.plan_allowed() && dropped.plan_declined);
    // Without `expires_in`, the access token's `exp`.
    let g = chatgpt::parse_grant(&resp(json!({"access_token": jwt(json!({"exp": 123}))}))).unwrap();
    assert_eq!(chatgpt::renewed(&c, &g, 0).expires_at, 123);
    // Refusals: unusable tokens, other codes, and nothing echoed from the reply.
    for code in ["invalid_grant", "refresh_token_reused", "refresh_token_expired", "token_expired", "invalid_refresh_token"] {
        let r = HttpResponse { status: 400, body: json!({"error": code}).to_string().into_bytes(), request_id: None };
        assert_eq!(chatgpt::parse_grant(&r), Err(chatgpt::GrantError::Unusable), "{code}");
    }
    assert!(chatgpt::GrantError::Unusable.message("renewing").contains("sign in again"));
    let hostile = HttpResponse {
        status: 400,
        body: br#"{"error":"synthetic-secret", "error_description":"synthetic-secret"}"#.to_vec(),
        request_id: None,
    };
    assert!(!format!("{:?}", chatgpt::parse_grant(&hostile)).contains("synthetic-secret"));
    let r = HttpResponse { status: 400, body: br#"{"error":"invalid_client"}"#.to_vec(), request_id: None };
    assert_eq!(chatgpt::parse_grant(&r), Err(chatgpt::GrantError::Other("invalid_client".into())));
    for value in [json!({}), json!({"refresh_token": "rotated"}), json!({"access_token": ""}), json!({"access_token": 42})] {
        assert!(chatgpt::parse_grant(&resp(value)).is_err());
    }
    assert!(
        chatgpt::parse_grant(&HttpResponse { status: 503, body: b"<html>".to_vec(), request_id: None })
            .unwrap_err()
            .message("x")
            .contains("HTTP 503")
    );
    // The OpenID configuration's endpoints must be on the sign-in service.
    let config =
        json!({"revocation_endpoint": "https://auth.openai.com/api/accounts/oauth/revoke", "jwks_uri": "https://evil.test/jwks.json"});
    assert_eq!(chatgpt::discovered(&config, "revocation_endpoint").as_deref(), Some("https://auth.openai.com/api/accounts/oauth/revoke"));
    assert!(chatgpt::discovered(&config, "jwks_uri").is_none());
    assert!(chatgpt::discovered(&json!({"jwks_uri": "https://auth.openai.com.evil.test/k"}), "jwks_uri").is_none());
    assert_eq!(
        chatgpt::query_params("code=a%2Fb&state=x+y&flag"),
        [("code".to_string(), "a/b".to_string()), ("state".into(), "x y".into()), ("flag".into(), String::new())]
    );
    assert_eq!(chatgpt::rfc3339(0), "1970-01-01T00:00:00Z");
    assert_eq!(chatgpt::rfc3339(1_760_448_856), "2025-10-14T13:34:16Z");
}

#[test]
fn chatgpt_id_token_claims() {
    let claims = json!({"iss": "https://auth.openai.com", "aud": "oaiapp_1", "exp": 2_000, "nonce": "n1", "sub": "user-1", "email": "ann@example.com"});
    let ok = chatgpt::check_id_claims(&claims, "oaiapp_1", "n1", 1_000).unwrap();
    assert_eq!((ok.subject.as_str(), ok.email.as_str()), ("user-1", "ann@example.com"));
    let mut array_aud = claims.clone();
    array_aud["aud"] = json!(["other", "oaiapp_1"]);
    assert!(chatgpt::check_id_claims(&array_aud, "oaiapp_1", "n1", 1_000).is_ok());
    // A little clock skew is allowed, no more.
    assert!(chatgpt::check_id_claims(&claims, "oaiapp_1", "n1", 2_000 + chatgpt::CLOCK_SKEW).is_ok());
    assert!(chatgpt::check_id_claims(&claims, "oaiapp_1", "n1", 2_001 + chatgpt::CLOCK_SKEW).is_err());
    for (k, v) in
        [("iss", json!("https://evil.test")), ("aud", json!("other")), ("nonce", json!("n2")), ("sub", json!("")), ("exp", json!(null))]
    {
        let mut bad = claims.clone();
        bad[k] = v;
        assert!(chatgpt::check_id_claims(&bad, "oaiapp_1", "n1", 1_000).is_err(), "{k}");
    }
    assert!(chatgpt::check_id_claims(&claims, "", "n1", 1_000).is_err());
    assert!(chatgpt::check_id_claims(&claims, "oaiapp_1", "", 1_000).is_err());
}

/// A synthetic RSA key (PKCS#8, made for these tests only) to sign id tokens with.
#[cfg(not(target_arch = "wasm32"))]
const TEST_RSA_KEY: &str = concat!(
    "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCRKBKS2ZKYuFIBTvqD/rNUHH7MkGXzylVRuOhfPKqagme8",
    "+9jcH7dXBHlxl5Nd3A4RffELcC9txKkxuXbLm7x4dhT3ayQkS3T9NFBtQr/cZ/GUuQR/+cV2YkwNAYlBewj1iP8dvYT2TTRS",
    "7nJ59TDnwzd88CRvushJJzEpCtMskBd7QnxTHLs/KxnWhavKsaw0uwA/3WE3Lm32Bhefp1Pxdv5NOOvxcrQsxcAPJh/uhz5w",
    "Goq/PaXlKA/luYQ3sNwQDhqx2i0TyDEBvCg3llIP0JgDW2NAikzNGk12E+qC0TzMaIb9jpx7VLux1/Yyg++Dt26aLusmpNwp",
    "fv+L13pJAgMBAAECggEACHj/pzVZP94iYJPXk4oeYDPO7ZR6yaFvHCIK447gS7x+NE25pBTwgqfENEPWBOMp/w4ML9AQifux",
    "/zEFehpRBJxVL6FABCOouc7VPaC+dNhXBYihy/EsUXNTX774iqs/zp/RzvlCSuN0Fmh3Th8TB7QhXQLiCD4tdUz7AEZIJTr9",
    "YqZafJcMsxMXjD7SCkPFSaHlv9cpDD2XJC3OCZxNsRHyBaFhv6ksHkmN6eOI0SQrBuyvWpb0ZVfN8aNNrR39YmQcJsloeHE1",
    "fpJ/npRXb1s1Q3qmyNZgLeC6oajQyxePnNVfqYJ7NWh2DpWhE1FSrz73SzCUyI7OcjXdFkcgcQKBgQDCio0ILB1kOgfjOojc",
    "N0MO2qaTNEcSG6E3dTJ8rwAn9QBiRPzGJLWsEdsXa4zYd5MTFk/TqgC9pg2yD2bl8U+QUlZ4x5K9DggKfdHPfHtL0MQW+xsF",
    "QVsamghs2jO+024Jn82qiUIXBQJeJA6sQ7xhdttitoyeYHgOlZQznEgUowKBgQC/A43Cejwyj7moVRHlAfSq6P22uM9yIDGG",
    "sQlaBRvAo+Hsoo2REsRGJ+vk/t+AFI6Gg9ab08RtABfqdLYQ6C0EsFJ0TapxamDm6L7VlI+aHr/Qcf6K6d67avTBISHyfrvN",
    "bQ1PR0Xeydziv5jy9asLvJ7uvspdpsspxfRNTzM4IwKBgG6l1R8v0CvAYdoY4dy2xbo+HH4fPskK1AAx9Vw7QNB5JhW+fTLJ",
    "kFbk9QL2Fe7M2+cASrjtuWwkdSewYOsp/AHvAuXBRTGoOGSXOW1UtN/jRpR3j9/GU2Xm9CHNXZ8MrEpnDNgRJ3jmTXlz8rCB",
    "+jRJIPCdOY+e+Mn6w4czPUAXAoGAVqRnBeLqMUkOPrOvtT/o2ilrBBnaRvshW+YrxsBwcTYlAXBOZJKtouDy2GFfwboRWCSv",
    "EOnCvGqBbqDYAN0/plPQYt7jxCQrpjHGdhy38oGmIgfDHQGuPTbrpDrCKjugNjMUCXTb1Y2c+uFLoFEc7+rz8HlsU4SXvPAE",
    "u5iENB8CgYEAogqjqgJbcBNriT4V0GRm+V8yeVQ00VYsbkbp9QN/6cB6OgXEUjn1iSrWQklljX4lFqcGohrt7NJ00xnVix37",
    "+GcoNYxSdNKQzXfYJMuc+FHxPat9gU6wD1ww3oPyfT4dX0GT45AVFuGgzlJn6yCd/AEGfT0mPNiWYGu/GJVYEcw=",
);

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn chatgpt_id_token_signature() {
    use base64::Engine;
    let b64 = |b: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
    let der = base64::engine::general_purpose::STANDARD.decode(TEST_RSA_KEY).unwrap();
    let key = ring::signature::RsaKeyPair::from_pkcs8(&der).unwrap();
    let public: ring::rsa::PublicKeyComponents<Vec<u8>> = key.public().into();
    let (n, e) = (public.n, public.e);
    let jwks = json!({"keys": [{"kty": "RSA", "alg": "RS256", "kid": "k1", "n": b64(&n), "e": b64(&e)}]});
    let sign = |header: Value, claims: Value| {
        let signed = format!("{}.{}", b64(header.to_string().as_bytes()), b64(claims.to_string().as_bytes()));
        let mut sig = vec![0; key.public().modulus_len()];
        key.sign(&ring::signature::RSA_PKCS1_SHA256, &ring::rand::SystemRandom::new(), signed.as_bytes(), &mut sig).unwrap();
        format!("{signed}.{}", b64(&sig))
    };
    let claims = json!({"iss": "https://auth.openai.com", "sub": "user-1"});
    let token = sign(json!({"alg": "RS256", "kid": "k1"}), claims.clone());
    assert_eq!(chatgpt::verify_jwt(&token, &jwks).unwrap(), claims);
    // A changed payload, another key, another algorithm, no published key.
    let (head, rest) = token.split_once('.').unwrap();
    let sig = rest.split_once('.').unwrap().1;
    let forged = format!("{head}.{}.{sig}", b64(json!({"iss": "https://auth.openai.com", "sub": "user-2"}).to_string().as_bytes()));
    assert!(chatgpt::verify_jwt(&forged, &jwks).is_err());
    let other = json!({"keys": [{"kty": "RSA", "kid": "k1", "n": b64(&[0xc3; 256]), "e": "AQAB"}]});
    assert!(chatgpt::verify_jwt(&token, &other).is_err());
    assert!(chatgpt::verify_jwt(&jwt(claims.clone()), &jwks).is_err(), "alg none");
    assert!(chatgpt::verify_jwt(&sign(json!({"alg": "RS256", "kid": "k2"}), claims), &jwks).is_err());
    for junk in ["", "a.b", "a.b.c", "....", "e30.e30.e30"] {
        assert!(chatgpt::verify_jwt(junk, &jwks).is_err(), "{junk}");
    }
}

#[test]
fn chatgpt_callback_requires_exact_unique_state() {
    for query in ["state=expected", "code=fake&state=expected"] {
        assert!(chatgpt::callback_state_matches(&chatgpt::query_params(query), "expected"));
    }
    for query in ["", "state=", "state=wrong", "state=expected-suffix", "state=expected&state=wrong", "state=expected&state=expected"] {
        assert!(!chatgpt::callback_state_matches(&chatgpt::query_params(query), "expected"), "{query}");
    }
    assert!(!chatgpt::callback_state_matches(&chatgpt::query_params("state="), ""));
}

#[test]
fn chatgpt_only_sends_credentials_to_the_openai_api() {
    assert!(chatgpt::api_url("https://api.openai.com/v1/responses"));
    assert!(chatgpt::api_url("https://api.openai.com/v1/models"));
    for url in [
        "http://api.openai.com/v1/responses",
        "https://api.openai.com.evil.test/v1/responses",
        "https://api.openai.com@evil.test/v1/responses",
        "https://api.openai.com/v1/responses@evil.test",
        "https://evil.test/v1/responses",
        "http://localhost:1234/v1/responses",
        "https://api.openai.com/v1/files",
        "https://api.openai.com/v1/responses/../files",
        "https://api.openai.com/v1-evil/responses",
        "https://api.openai.com/v1/responses\\evil.test",
        "https://api.openai.com/v1/responses#fragment",
        "https://chatgpt.com/backend-api/codex/responses",
    ] {
        assert!(!chatgpt::api_url(url), "{url}");
        // This check runs before any credential file is loaded or a network request is made.
        #[cfg(not(target_arch = "wasm32"))]
        assert!(
            chatgpt::send(&HttpRequest { method: "GET", url: url.into(), headers: vec![], body: None }, &chatgpt::Binding::default())
                .unwrap_err()
                .to_string()
                .contains("only")
        );
    }
}

#[test]
fn responses_reads_crlf_event_stream() {
    let item = json!({"type":"message", "content":[{"type":"output_text", "text":"CRLF answer"}]});
    let mut response = sse(&[item], "completed").unwrap();
    response.body = String::from_utf8(response.body).unwrap().replace("\n", "\r\n").into_bytes();
    assert_eq!(parse_reply(Api::Responses, &response).unwrap().text, "CRLF answer");
}

#[test]
fn responses_refusal_does_not_replay_unanswered_calls() {
    let refusal = json!({"type":"message", "content":[{"type":"refusal", "refusal":"declined"}]});
    let call = json!({"type":"function_call", "call_id":"ignored", "name":"run_command", "arguments":"{}"});
    let reasoning = json!({"type": "reasoning", "summary": [], "encrypted_content": "gAAAA-refused"});
    let reply = parse_reply(Api::Responses, &sse(&[reasoning, refusal, call], "completed").unwrap()).unwrap();
    assert!(reply.calls.is_empty());
    assert!(!reply.native.as_array().unwrap().iter().any(|item| item["type"] == "function_call" || item["type"] == "reasoning"));
    assert!(reply.native.as_array().unwrap().iter().any(|item| item["type"] == "message"), "the refusal itself is kept");
}

#[test]
fn responses_replay_follows_output_index() {
    let body = concat!(
        "data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"answer\"}]}}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"reasoning\",\"encrypted_content\":\"synthetic\"}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n",
        "data: [DONE]\n\n"
    );
    let reply = parse_reply(Api::Responses, &HttpResponse { status: 200, body: body.as_bytes().to_vec(), request_id: None }).unwrap();
    assert_eq!(reply.native[0]["type"], "reasoning");
    assert_eq!(reply.native[1]["type"], "message");
    assert_eq!(reply.text, "answer");
}

#[test]
fn incomplete_responses_never_execute_or_replay_calls() {
    let message = json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Partial answer"}]});
    let call = json!({"type": "function_call", "call_id": "cut", "name": "run_command",
        "arguments": json!({"command": "map.goToXY", "params": {"lon": 2.35, "lat": 48.85}}).to_string()});
    // Reasoning is refused without the item that followed it, so it goes with the calls.
    let reasoning = json!({"type": "reasoning", "summary": [], "encrypted_content": "gAAAA-cut"});
    let mut responses = vec![sse(&[reasoning.clone(), message.clone(), call.clone()], "incomplete").unwrap()];
    for status in ["incomplete", "failed", "in_progress", "cancelled", "unknown", ""] {
        responses.push(HttpResponse {
            status: 200,
            request_id: None,
            body: json!({"status": status, "output": [reasoning, message, call]}).to_string().into_bytes(),
        });
    }
    for response in responses {
        let parsed = parse_reply(Api::Responses, &response).unwrap();
        assert_eq!(parsed.text, "Partial answer");
        assert!(parsed.calls.is_empty() && parsed.cut.is_some());
        assert!(!parsed.native.as_array().unwrap().iter().any(|v| v["type"] == "function_call" || v["type"] == "reasoning"));
        let mut session = sample();
        let before = session.map().center_lonlat();
        let mut agent = Agent::new(chatgpt_cfg(), &session);
        agent.ask("move", &session);
        assert_eq!(agent.receive(agent.generation(), Ok(response), &mut session), Step::Done);
        assert_eq!(session.map().center_lonlat(), before);
        let Step::Send(next) = agent.ask("continue", &session) else { panic!("request") };
        assert!(!body(&next)["input"].as_array().unwrap().iter().any(|v| v["type"] == "function_call" || v["type"] == "reasoning"));
    }
}

#[test]
fn catalog_defaults_names_and_reasoning_capabilities() {
    let response = HttpResponse {
        status: 200,
        request_id: None,
        body: json!({"models": [
            {"slug":"plain", "display_name":"Plain model", "visibility":"list", "supported_reasoning_levels":[]},
            {"slug":"think", "visibility":"list", "supports_reasoning":true},
            {"slug":"legacy", "visibility":"list"}, {"slug":"hidden", "visibility":"hide"}
        ]})
        .to_string()
        .into_bytes(),
    };
    let models = parse_catalog(&response).unwrap();
    assert_eq!(models.len(), 3);
    assert_eq!(models[0].name, "Plain model");
    let mut cfg = chatgpt_cfg();
    use_catalog(&mut cfg, &models, false);
    assert_eq!(cfg.model, "plain");
    let request = chat_request(&cfg, "hello", &[], &[]);
    assert!(body(&request).get("reasoning").is_none());
    assert!(body(&request).get("include").is_none());
    for model in ["think", "legacy", "explicit-unlisted"] {
        cfg.model = model.into();
        use_catalog(&mut cfg, &models, true);
        assert_eq!(cfg.model, model);
        assert!(body(&chat_request(&cfg, "hello", &[], &[])).get("reasoning").is_some());
    }
}

#[test]
fn structured_errors_keep_diagnostics_and_correct_recovery() {
    use crate::Recovery;
    for (code, recovery) in [
        ("chatpass_v2_scope_not_authorized", Recovery::Configuration),
        ("chatpass_v2_invalid_authorization_context", Recovery::Configuration),
        ("subscription_sharing_invalid_user", Recovery::Configuration),
        ("subscription_sharing_usage_limit_exceeded", Recovery::Usage),
        ("subscription_sharing_usage_unavailable", Recovery::Later),
        ("subscription_sharing_user_not_eligible", Recovery::Ineligible),
        ("subscription_sharing_unsupported_capability", Recovery::Unsupported),
    ] {
        let response = HttpResponse {
            status: 403,
            request_id: Some("request-123".into()),
            body: json!({"error": {"code":code,"param":"tools[0]","message":"diagnostic"}}).to_string().into_bytes(),
        };
        let error = parse_reply(Api::Responses, &response).unwrap_err();
        assert_eq!(error.recovery, recovery);
        assert_eq!(error.code.as_deref(), Some(code));
        assert_eq!(error.param.as_deref(), Some("tools[0]"));
        assert_eq!(error.request_id.as_deref(), Some("request-123"));
        assert_eq!(error.status, Some(403));
        assert_eq!(error.shape, "error");
        if recovery == Recovery::Configuration {
            assert!(!error.message().contains("sign in again"));
        }
    }
}

#[test]
fn http_debug_never_contains_secrets() {
    let request = HttpRequest {
        method: "POST",
        url: "https://example.test?key=url-secret".into(),
        headers: vec![("AUTHORIZATION".into(), "bearer-secret".into()), ("x-api-key".into(), "key-secret".into())],
        body: Some("refresh_token=body-secret".into()),
    };
    let debug = format!("{request:?}");
    for secret in ["url-secret", "bearer-secret", "key-secret", "body-secret"] {
        assert!(!debug.contains(secret));
    }
    let response = HttpResponse { status: 200, request_id: None, body: b"token-secret".to_vec() };
    assert!(!format!("{response:?}").contains("token-secret"));
}

#[test]
fn non_json_plan_errors_keep_status_request_id_and_recovery() {
    for (status, recovery) in [(403, crate::Recovery::Configuration), (503, crate::Recovery::Later)] {
        let response = HttpResponse { status, request_id: Some("admission-request".into()), body: b"upstream admission failed".to_vec() };
        for error in [parse_catalog(&response).unwrap_err(), parse_reply(Api::Responses, &response).unwrap_err()] {
            assert_eq!(error.status, Some(status));
            assert_eq!(error.request_id.as_deref(), Some("admission-request"));
            assert_eq!(error.recovery, recovery);
            assert_eq!(error.shape, "other");
            assert_eq!(error.detail, "upstream admission failed");
        }
    }
}

#[test]
fn plain_failed_response_preserves_partial_text_and_usage_recovery() {
    let response = HttpResponse {
        status: 200,
        request_id: Some("request-failed".into()),
        body: json!({
            "status": "failed", "error": {"code": "subscription_sharing_usage_limit_exceeded"},
            "output": [{"type":"message", "content":[{"type":"output_text", "text":"Partial"}]},
                {"type":"function_call", "name":"run_command", "arguments":"{}"}]
        })
        .to_string()
        .into_bytes(),
    };
    let parsed = parse_reply(Api::Responses, &response).unwrap();
    assert_eq!(parsed.text, "Partial");
    assert!(parsed.cut.is_some() && parsed.calls.is_empty());
    assert_eq!(parsed.failure.unwrap().recovery, crate::Recovery::Usage);
}

#[test]
fn token_errors_keep_codes_and_do_not_echo_credentials() {
    for (code, recovery) in [("invalid_client", crate::Recovery::Configuration), ("invalid_grant", crate::Recovery::SignIn)] {
        let response = HttpResponse {
            status: 400,
            request_id: Some("oauth-request".into()),
            body: json!({"error": code, "error_description": "synthetic-secret"}).to_string().into_bytes(),
        };
        let error = crate::Error::token(&response, chatgpt::parse_grant(&response).unwrap_err());
        assert_eq!(error.code.as_deref(), Some(code));
        assert_eq!(error.request_id.as_deref(), Some("oauth-request"));
        assert_eq!(error.status, Some(400));
        assert_eq!(error.recovery, recovery);
        assert!(!format!("{error:?}").contains("synthetic-secret"));
    }
}
