//! RustGIS on the command line. Every command the ribbon runs is available here by id.
//!
//! ```text
//! rustgis-cli commands                                  # list command ids and parameters
//! rustgis-cli tools                                     # list geoprocessing tools
//! rustgis-cli info data.geojson                         # layer summary as JSON
//! rustgis-cli render data.geojson out.png [--basemap ocean] [--tiles URL] [--width 1600 --height 900]
//! rustgis-cli run [--sample | --project p.rgis | --data f.geojson …] \
//!     --cmd 'gp.run={"tool":"buffer","params":{"input":"Cities","distance":100}}' \
//!     --cmd 'share.exportMap={"path":"out.png"}' [--save out.rgis]
//! rustgis-cli ask [--sample | --project p.rgis | --data f.geojson …] "Which cities have over 10 million people?" \
//!     [--provider anthropic|openai|chatgpt|gemini|openrouter|ollama|custom] [--model M] [--base-url URL] [--yes] [--save out.rgis] [--trace log.txt]
//! rustgis-cli ask --models [--provider P]                # list the provider's models
//! rustgis-cli ask --accounts | --account CLIENT_ID       # list or select a saved registration
//! rustgis-cli ask --login [--another-account]            # sign in with ChatGPT to use your ChatGPT plan
//! rustgis-cli ask --logout
//! rustgis-cli mcp [--sample | --project p.rgis | --data f.geojson …] [--yes]   # an MCP server on stdio
//! rustgis-cli mcp --attach [PORT] [--yes]   # the same, on the desktop app already open (its MCP Relay)
//! rustgis-cli plugins [install <file|folder> | uninstall <id> | enable <id> | disable <id>
//!                      | browse | get <id> | prepare <id>] [--registry URL]
//! ```
//!
//! Installed plugins are loaded first (`--no-plugins` skips them); `--plugin <file|folder>`
//! loads one more for this run only, so its commands (`plugin:<id>/<command>`) and tools can
//! be run with `--cmd`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write;
use std::process::ExitCode;

/// `println!` that ignores a closed pipe (`rustgis-cli commands | head`).
macro_rules! out {
    ($($t:tt)*) => {{
        let _ = writeln!(std::io::stdout(), $($t)*);
    }};
}

use rustgis_engine::{Project, Session};
use serde_json::{Value, json};

fn usage() -> ExitCode {
    eprintln!(
        "usage:\n  rustgis-cli commands | tools\n  rustgis-cli plugins [install <file|folder> | uninstall <id> | enable <id> | disable <id> | browse | get <id> | prepare <id>] [--registry URL]\n  rustgis-cli info <data>\n  rustgis-cli logo <out.png|out.ico|out.bmp> [--size N] [--canvas WxH --at X,Y (.bmp only)]\n  rustgis-cli render <data|project> <out.png> [--basemap NAME] [--tiles URL] [--width N] [--height N] [--dpi N] [--worldview auto|default|china]\n  rustgis-cli run [--sample] [--project P] [--data F]… --cmd 'id={{json}}'… [--save P] [--worldview auto|default|china]\n  rustgis-cli ask [--sample] [--project P] [--data F]… \"request\" [--provider P] [--model M] [--base-url URL] [--yes] [--save P]\n  rustgis-cli ask --models [--provider P]\n  rustgis-cli ask --login [--another-account] [--account CLIENT_ID] | --logout\n  rustgis-cli ask --accounts | --account CLIENT_ID\n  rustgis-cli mcp [--sample] [--project P] [--data F]… [--yes] [--attach [PORT]]"
    );
    ExitCode::from(2)
}

fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(|s| s.as_str())
}

fn print(v: &Value) {
    out!("{}", serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()));
}

/// Runs a command; what it said beside its result (the app's status bar) goes to standard error.
fn run(s: &mut Session, id: &str, p: Value) -> Result<Value, String> {
    let mark = s.notice_mark();
    let r = s.run(id, &p);
    // A failure is noted too; it's printed once, by the caller.
    for n in s.notices_since(mark).iter().filter(|n| r.as_ref().err() != Some(&n.text)) {
        eprintln!("rustgis-cli: {}{}", if n.error { "error: " } else { "" }, n.text);
    }
    r.map_err(|e| format!("{id}: {e}"))
}

/// Makes a Python plugin's environment, saying what it's doing as it goes.
fn prepare_env(id: &str) -> Result<String, String> {
    eprintln!("rustgis-cli: preparing {id}'s Python environment (the first time downloads Python and its packages)");
    let python = rustgis_engine::plugins::pyenv::prepare(id, &mut |line| eprintln!("rustgis-cli: {line}"))?;
    Ok(python.display().to_string())
}

fn is_project(path: &str) -> bool {
    rustgis_engine::project::is_project_file(path) || path.to_ascii_lowercase().ends_with(".rgispkg")
}

fn num(args: &[String], name: &str) -> Option<f64> {
    opt(args, name).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite())
}

fn load(s: &mut Session, path: &str) -> Result<(), String> {
    if is_project(path) {
        run(s, "project.open", json!({"path": path}))?;
    } else {
        run(s, "layer.addData", json!({"path": path}))?;
    }
    Ok(())
}

/// Opens `--sample`, `--project` and every `--data`.
fn open_inputs(s: &mut Session, args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--sample") {
        run(s, "project.sample", Value::Null)?;
    }
    if let Some(p) = opt(args, "--project") {
        load(s, p)?;
    }
    for (i, a) in args.iter().enumerate() {
        if a == "--data"
            && let Some(p) = args.get(i + 1)
        {
            load(s, p)?;
        }
    }
    Ok(())
}

/// `ask "<request>"`: one conversation turn with the AI assistant, which acts on the project.
fn ask(s: &mut Session, args: &[String]) -> Result<(), String> {
    use rustgis_assistant::{Agent, Config, Entry, agent, http, provider};
    // The first value that isn't an option's.
    let takes_value = [
        "--provider",
        "--model",
        "--base-url",
        "--project",
        "--data",
        "--save",
        "--trace",
        "--plugin",
        "--registry",
        "--worldview",
        "--account",
    ];
    let prompt = args
        .iter()
        .enumerate()
        .skip(1)
        .find(|(i, a)| !a.starts_with("--") && !args.get(i - 1).is_some_and(|p| takes_value.contains(&p.as_str())))
        .map(|(_, a)| a.clone());
    let provider = opt(args, "--provider")
        .map(str::to_string)
        .or_else(|| std::env::var("RUSTGIS_ASSISTANT_PROVIDER").ok())
        .or_else(|| {
            // The first provider with a key in the environment (a ChatGPT sign-in is chosen, not assumed).
            provider::PRESETS.iter().find(|p| !p.login && !Config::from_env(p.id).key.is_empty()).map(|p| p.id.to_string())
        })
        .unwrap_or_else(|| "ollama".into());
    if provider::preset(&provider).is_none() {
        let ids: Vec<&str> = provider::PRESETS.iter().map(|p| p.id).collect();
        return Err(format!("unknown provider '{provider}' (one of {})", ids.join(", ")));
    }
    if args.iter().any(|a| a == "--accounts") {
        for c in rustgis_assistant::chatgpt::accounts()? {
            out!("{}\t{}", c.client_id, c.label());
        }
        return Ok(());
    }
    if let Some(id) = opt(args, "--account") {
        let accounts = rustgis_assistant::chatgpt::accounts()?;
        let matches: Vec<_> = accounts.iter().filter(|c| c.client_id == id).collect();
        if matches.len() != 1 {
            return Err("--account needs an unambiguous client id from --accounts".into());
        }
        if let Some(c) = matches.first() {
            rustgis_assistant::chatgpt::select(&c.client_id, &c.subject)?;
        }
        if prompt.is_none() && !args.iter().any(|a| a == "--login" || a == "--models") {
            return Ok(());
        }
    }
    let mut cfg = Config::from_env(&provider);
    if let Some(m) = opt(args, "--model").map(str::to_string).or_else(|| std::env::var("RUSTGIS_ASSISTANT_MODEL").ok()) {
        cfg.model = m;
    }
    if let Some(u) = opt(args, "--base-url") {
        cfg.base_url = u.to_string();
    }
    if args.iter().any(|a| a == "--logout") {
        use rustgis_assistant::chatgpt;
        if let Some(r) = chatgpt::sign_out()?
            && let Err(e) = chatgpt::revoke(&r)
        {
            eprintln!(
                "signed out on this computer, but OpenAI didn't confirm it ({e}); disconnect RustGIS in ChatGPT's settings if needed"
            );
            return Ok(());
        }
        eprintln!("signed out of ChatGPT");
        return Ok(());
    }
    if args.iter().any(|a| a == "--login") {
        use rustgis_assistant::chatgpt;
        let login = chatgpt::login(args.iter().any(|a| a == "--another-account"))?;
        eprintln!("Open this page in your browser to continue with ChatGPT:\n{}", login.url);
        let c = loop {
            if let Some(r) = login.poll() {
                break r?;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        };
        if !c.plan_allowed() {
            return Err(rustgis_assistant::Error::new(rustgis_assistant::Recovery::Consent, "").to_string());
        }
        let who = if c.email.is_empty() { "your ChatGPT account" } else { c.email.as_str() };
        eprintln!("signed in as {who}; RustGIS now uses your ChatGPT plan (manage usage at {})", chatgpt::USAGE_URL);
        return Ok(());
    }
    // A signed-in account's requests renew its tokens as needed.
    // Without a usable sign-in, say how to sign in before asking for anything.
    if cfg.login()
        && cfg.key.is_empty()
        && (prompt.is_some() || args.iter().any(|a| a == "--models"))
        && let Some(why) = cfg.missing()
    {
        return Err(format!("{provider}: {why} (run rustgis-cli ask --login)"));
    }
    let login = cfg.login().then(|| cfg.binding());
    let signed_in = login.is_some();
    let transport = move |req: &rustgis_assistant::HttpRequest| match &login {
        Some(account) => rustgis_assistant::chatgpt::send(req, account).map_err(String::from),
        None => http::send(req),
    };
    if signed_in && !args.iter().any(|a| a == "--models") && prompt.is_some() {
        let r = transport(&provider::models_request(&cfg))?;
        let models = provider::parse_catalog(&r)?;
        if models.is_empty() {
            return Err("no models are available for this account".into());
        }
        let explicit = opt(args, "--model").is_some() || std::env::var("RUSTGIS_ASSISTANT_MODEL").is_ok();
        if !explicit {
            cfg.model.clear();
        }
        provider::use_catalog(&mut cfg, &models, explicit);
    }
    if args.iter().any(|a| a == "--models") {
        let r = transport(&provider::models_request(&cfg))?;
        if signed_in {
            for m in provider::parse_catalog(&r)? {
                out!("{}\t{}", m.id, m.name);
            }
        } else {
            for m in provider::parse_models(cfg.api(), &r)? {
                out!("{m}");
            }
        }
        return Ok(());
    }
    let prompt = prompt.ok_or("ask needs a request, e.g. rustgis-cli ask --sample \"Which countries have the largest GDP?\"")?;
    if let Some(why) = cfg.missing() {
        let hint = match provider::preset(&provider) {
            Some(p) if p.login => "run rustgis-cli ask --login".to_string(),
            Some(p) => format!("set {}", p.key_env.join(" or ")),
            None => String::new(),
        };
        return Err(format!("{provider}: {why} ({hint})"));
    }
    eprintln!("[{} · {}]", cfg.provider, cfg.model);
    let yes = args.iter().any(|a| a == "--yes");
    let mut a = Agent::new(cfg, s);
    let mut shown = 0;
    let mut show = |a: &Agent| {
        for e in a.transcript.iter().skip(shown) {
            match e {
                Entry::User(_) => {}
                Entry::Assistant(t) => out!("{t}"),
                Entry::Tool { summary, result, error, .. } => {
                    let r: String = result.chars().take(200).collect();
                    eprintln!("  {} {summary} → {r}", if *error { "✗" } else { "✓" });
                }
                Entry::Error(e) => eprintln!("rustgis-cli: {e}"),
                Entry::Note(n) => eprintln!("({n})"),
            }
        }
        shown = a.transcript.len();
    };
    // `--trace FILE`: each request and response body (never the headers, which carry the key).
    let trace = opt(args, "--trace").map(std::path::PathBuf::from);
    let send = |req: &rustgis_assistant::HttpRequest| {
        let r = transport(req);
        if let Some(path) = &trace {
            let resp = match &r {
                Ok(r) => format!("HTTP {}\n{}", r.status, String::from_utf8_lossy(&r.body)),
                Err(e) => e.clone(),
            };
            let entry = format!("> {} {}\n{}\n< {resp}\n\n", req.method, req.url, req.body.as_deref().unwrap_or(""));
            let written = std::fs::OpenOptions::new().create(true).append(true).open(path).and_then(|mut f| f.write_all(entry.as_bytes()));
            if let Err(e) = written {
                eprintln!("rustgis-cli: {}: {e}", path.display());
            }
        }
        r
    };
    // `--yes` allows file and network access; actions reserved to the user (plugins, scripts,
    // tasks, models, replacing the project) are never run from here.
    agent::run_blocking(&mut a, &prompt, s, send, |what, why, access| {
        let allow = yes && access;
        if !allow {
            let hint = if access { "; pass --yes to allow" } else { "" };
            eprintln!("  declined: {what} ({why}{hint})");
        }
        allow
    });
    show(&a);
    if a.transcript.iter().any(|e| matches!(e, Entry::Error(_))) {
        return Err("the assistant couldn't finish".into());
    }
    Ok(())
}

/// `--attach [PORT]` as `mcp` takes it: the port that follows when it isn't another flag, else
/// the app's default.
fn attach_port(args: &[String]) -> Result<Option<u16>, String> {
    let Some(i) = args.iter().position(|a| a == "--attach") else { return Ok(None) };
    match args.get(i + 1).filter(|a| !a.starts_with("--")) {
        Some(p) => p.parse::<u16>().map(Some).map_err(|_| format!("mcp --attach {p}: expected a port number")),
        None => Ok(Some(rustgis_assistant::relay::DEFAULT_PORT)),
    }
}

/// `mcp --attach [PORT]`: the same MCP server, on the desktop app that is already open. The
/// app's session runs the calls (its MCP Relay command started the listener on 127.0.0.1); this
/// process relays the protocol only. `--yes` goes along with the connection, so the app applies
/// the same file and network refusals as the private server does here.
fn attach(port: u16, allow_access: bool) -> Result<(), String> {
    // The relay wrote the run's token where only this user can read it; carrying it in the
    // attach message is what proves this is the user's own client, so nothing is pasted.
    let token = rustgis_assistant::relay::read_token(port)?;
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port))
        .map_err(|e| format!("nothing is listening on 127.0.0.1:{port} ({e}); start RustGIS's MCP Relay command first"))?;
    let hello = json!({"jsonrpc": "2.0", "method": "rustgis/attach", "params": {"access": allow_access, "token": token}});
    writeln!(stream, "{hello}").map_err(|e| format!("telling the app what --yes allows: {e}"))?;
    eprintln!(
        "rustgis-cli: MCP server ready (the open RustGIS app at 127.0.0.1:{port}; file and network access {})",
        if allow_access { "allowed" } else { "off, --yes allows it" }
    );
    let mut input = std::io::BufReader::new(stream.try_clone().map_err(|e| format!("opening the app's replies: {e}"))?);
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    loop {
        let line = match rustgis_assistant::framing::read_message(&mut stdin, rustgis_assistant::framing::Oversize::Skip) {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(why) => {
                let e = json!({"jsonrpc": "2.0", "id": Value::Null, "error": {"code": -32700, "message": read_error(why, "reading standard input")}});
                let _ = writeln!(stdout, "{e}").and_then(|()| stdout.flush());
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        writeln!(stream, "{line}").and_then(|()| stream.flush()).map_err(|e| format!("sending to the app: {e}"))?;
        if !rustgis_assistant::mcp::expects_reply(&line) {
            continue;
        }
        let reply = match rustgis_assistant::framing::read_message(&mut input, rustgis_assistant::framing::Oversize::Stop) {
            Ok(Some(reply)) => reply,
            Ok(None) => return Err("the RustGIS app closed the relay".into()),
            Err(why) => return Err(format!("the app's reply: {why}")),
        };
        writeln!(stdout, "{reply}").and_then(|()| stdout.flush()).map_err(|e| format!("writing to standard output: {e}"))?;
    }
    Ok(())
}

/// The complaint for a message that could not be read: the protocol's own words about the
/// message, or the failing reader named by its caller (the framing helper's I/O text is
/// neutral and names no source).
fn read_error(why: rustgis_assistant::framing::Error, source: &str) -> String {
    match why {
        rustgis_assistant::framing::Error::Io(e) => format!("{source}: {e}"),
        other => other.to_string(),
    }
}

/// `mcp`: a Model Context Protocol server on standard input and output, one JSON-RPC message per
/// line, until the client closes its end. Only messages go to standard output; notes go to
/// standard error. `--attach [PORT]` serves the same protocol on the desktop app's session
/// instead of a private one.
fn mcp(s: &mut Session, args: &[String]) -> Result<(), String> {
    let allow_access = args.iter().any(|a| a == "--yes");
    if let Some(port) = attach_port(args)? {
        if args.iter().any(|a| matches!(a.as_str(), "--sample" | "--project" | "--data")) {
            return Err("mcp --attach manages the project open in the app; --sample, --project and --data make a private one".into());
        }
        return attach(port, allow_access);
    }
    let server = rustgis_assistant::mcp::Server::new(allow_access);
    eprintln!(
        "rustgis-cli: MCP server ready ({} layers; file and network access {})",
        s.map().layers.len(),
        if server.allow_access { "allowed" } else { "off, --yes allows it" }
    );
    // The longest message read (a project's JSON can be large); longer lines are refused.
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    loop {
        let refuse = |stdout: &mut std::io::StdoutLock, why: &str| {
            let e = json!({"jsonrpc": "2.0", "id": Value::Null, "error": {"code": -32700, "message": why}});
            writeln!(stdout, "{e}").and_then(|()| stdout.flush()).is_ok()
        };
        let line = match rustgis_assistant::framing::read_message(&mut stdin, rustgis_assistant::framing::Oversize::Skip) {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(why) => {
                if !refuse(&mut stdout, &read_error(why, "reading standard input")) {
                    break;
                }
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        // A last resort: a panic answers that request with an error instead of ending the server.
        let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| server.handle(s, &line))).unwrap_or_else(|_| {
            // The panic message is already on standard error; the id lets the client match the error.
            eprintln!("rustgis-cli: a request failed unexpectedly; the project may be partly changed");
            let id = serde_json::from_str::<Value>(&line)
                .ok()
                .and_then(|v| v.get("id").cloned())
                .filter(|id| id.is_string() || id.is_number())
                .unwrap_or(Value::Null);
            Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": "internal error"}}).to_string())
        });
        if let Some(reply) = reply {
            // The client hung up.
            if writeln!(stdout, "{reply}").and_then(|()| stdout.flush()).is_err() {
                break;
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(|s| s.as_str()) else { return usage() };
    if !args.iter().any(|a| a == "--no-plugins") {
        for e in rustgis_engine::cmd::plugins::load_installed() {
            eprintln!("rustgis-cli: warning: {e}");
        }
    }
    for (i, a) in args.iter().enumerate() {
        if a == "--plugin"
            && let Some(p) = args.get(i + 1)
        {
            let loaded =
                rustgis_engine::plugins::Package::from_path(std::path::Path::new(p)).and_then(|pkg| rustgis_engine::plugins::load(pkg, p));
            if let Err(e) = loaded {
                eprintln!("rustgis-cli: {p}: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    // Whose borders the built-in countries and world outline show: the system's by default.
    let worldview = opt(&args, "--worldview").unwrap_or("auto");
    if worldview != "auto" && rustgis_engine::sample::Worldview::from_code(worldview).is_none() {
        eprintln!("rustgis-cli: --worldview {worldview}: expected auto, default or china");
        return ExitCode::from(2);
    }
    rustgis_engine::sample::apply_worldview(worldview);
    let mut s = Session::new(Project::default());
    let res: Result<(), String> = (|| match cmd {
        "plugins" => {
            let registry = opt(&args, "--registry").map(|u| json!(u)).unwrap_or(Value::Null);
            let arg = |what: &str| args.get(2).filter(|a| !a.starts_with("--")).cloned().ok_or(format!("plugins {what} needs an argument"));
            let out = match args.get(1).map(String::as_str) {
                None | Some("list") => run(&mut s, "plugin.list", Value::Null)?,
                Some("install") => run(&mut s, "plugin.install", json!({"path": arg("install")?}))?,
                Some("uninstall") => run(&mut s, "plugin.uninstall", json!({"id": arg("uninstall")?}))?,
                Some("enable") => run(&mut s, "plugin.enable", json!({"id": arg("enable")?, "enabled": true}))?,
                Some("disable") => run(&mut s, "plugin.enable", json!({"id": arg("disable")?, "enabled": false}))?,
                Some("browse") => run(&mut s, "plugin.registry", json!({"url": registry}))?,
                Some("get") => run(&mut s, "plugin.installFromRegistry", json!({"id": arg("get")?, "url": registry}))?,
                Some("prepare") => json!({"python": prepare_env(&arg("prepare")?)?}),
                Some(other) => return Err(format!("unknown plugins subcommand '{other}'")),
            };
            // A Python plugin just installed gets its environment now, so it runs.
            if matches!(args.get(1).map(String::as_str), Some("install" | "get"))
                && let Some(id) = out.get("id").and_then(Value::as_str)
                && rustgis_engine::plugins::pyenv::status(id) == Some(false)
            {
                prepare_env(id)?;
            }
            print(&out);
            Ok(())
        }
        "commands" => {
            print(&run(&mut s, "info.commands", Value::Null)?);
            Ok(())
        }
        "tools" => {
            print(&run(&mut s, "gp.tools", Value::Null)?);
            Ok(())
        }
        "info" => {
            let path = args.get(1).ok_or("info needs a file")?;
            load(&mut s, path)?;
            print(&run(&mut s, "info.project", Value::Null)?);
            Ok(())
        }
        "logo" => {
            // The app icon as a PNG (or an .ico for the Windows installers), for installers and
            // desktop entries.
            let out = args.get(1).filter(|a| !a.starts_with("--")).ok_or("logo needs <out.png> first")?;
            let lower = out.to_ascii_lowercase();
            let size = match opt(&args, "--size") {
                Some(v) => v.parse::<u32>().map_err(|_| format!("--size {v}: expected a whole number"))?,
                None => 512,
            }
            .clamp(16, 1024);
            if !lower.ends_with(".bmp") && (opt(&args, "--canvas").is_some() || opt(&args, "--at").is_some()) {
                return Err("--canvas and --at apply to a .bmp only".into());
            }
            if lower.ends_with(".ico") {
                // Every icon size from 16 to 256 px: --size does not apply.
                let ico = rustgis_engine::render::logo::ico().ok_or("could not draw the logo")?;
                std::fs::write(out, ico).map_err(|e| format!("{out}: {e}"))?;
                return Ok(());
            }
            if lower.ends_with(".bmp") {
                // An installer wizard image: the logo on a white canvas (--canvas WxH, default
                // the logo's size), at --at X,Y (default centred).
                let pair = |name: &str, sep: char| -> Result<Option<(i64, i64)>, String> {
                    let Some(v) = opt(&args, name) else { return Ok(None) };
                    let bad = || format!("{name} {v}: expected two whole numbers like 10{sep}20");
                    let (a, b) = v.split_once(sep).ok_or_else(bad)?;
                    Ok(Some((a.trim().parse().map_err(|_| bad())?, b.trim().parse().map_err(|_| bad())?)))
                };
                let (w, h) = pair("--canvas", 'x')?.unwrap_or((i64::from(size), i64::from(size)));
                let (w, h) = (w.clamp(1, 2048) as u32, h.clamp(1, 2048) as u32);
                let (x, y) = pair("--at", ',')?.unwrap_or(((i64::from(w) - i64::from(size)) / 2, (i64::from(h) - i64::from(size)) / 2));
                let (x, y) = (x.clamp(-4096, 4096) as i32, y.clamp(-4096, 4096) as i32);
                let bmp = rustgis_engine::render::logo::bmp(size, w, h, x, y).ok_or("could not draw the logo")?;
                std::fs::write(out, bmp).map_err(|e| format!("{out}: {e}"))?;
                return Ok(());
            }
            let png =
                rustgis_engine::render::logo::pixmap(size).ok_or("could not draw the logo")?.encode_png().map_err(|e| e.to_string())?;
            std::fs::write(out, png).map_err(|e| format!("{out}: {e}"))?;
            Ok(())
        }
        "render" => {
            let (Some(input), Some(out)) = (args.get(1), args.get(2)) else { return Err("render needs <input> <out.png>".into()) };
            load(&mut s, input)?;
            if let Some(b) = opt(&args, "--basemap") {
                run(&mut s, "map.basemap", json!({"name": b}))?;
            }
            // A web tile basemap: raster tiles, vector tiles or a MapLibre style by address.
            if let Some(u) = opt(&args, "--tiles") {
                run(&mut s, "map.addTiles", json!({"url": u}))?;
            }
            let w = num(&args, "--width").unwrap_or(1600.0).clamp(16.0, 8000.0);
            let h = num(&args, "--height").unwrap_or(900.0).clamp(16.0, 8000.0);
            s.view_size = (w, h);
            if !is_project(input) {
                run(&mut s, "map.zoomFull", Value::Null)?;
            }
            let dpi = num(&args, "--dpi").unwrap_or(96.0);
            print(&run(&mut s, "share.exportMap", json!({"path": out, "dpi": dpi}))?);
            Ok(())
        }
        "ask" => {
            open_inputs(&mut s, &args)?;
            // What the assistant did is saved even when it couldn't finish.
            let asked = ask(&mut s, &args);
            if let Some(p) = opt(&args, "--save")
                && !args.iter().any(|a| a == "--models")
            {
                run(&mut s, "project.saveAs", json!({"path": p}))?;
            }
            asked
        }
        "mcp" => {
            if !args.iter().any(|a| a == "--attach") {
                open_inputs(&mut s, &args)?;
            }
            mcp(&mut s, &args)
        }
        "run" => {
            open_inputs(&mut s, &args)?;
            for (i, a) in args.iter().enumerate() {
                if a == "--cmd"
                    && let Some(c) = args.get(i + 1)
                {
                    let (id, p) = match c.split_once('=') {
                        Some((id, json)) => (id, serde_json::from_str(json).map_err(|e| format!("{id}: bad JSON: {e}"))?),
                        None => (c.as_str(), Value::Null),
                    };
                    let out = run(&mut s, id, p)?;
                    if !out.is_null() {
                        print(&out);
                    }
                }
            }
            if let Some(p) = opt(&args, "--save") {
                run(&mut s, "project.saveAs", json!({"path": p}))?;
            }
            Ok(())
        }
        _ => Err(format!("unknown subcommand '{cmd}'")),
    })();
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rustgis-cli: {e}");
            ExitCode::FAILURE
        }
    }
}
