//! The AI Assistant pane: a chat with a model of the user's choosing, which acts on the map by
//! running commands (see the `rustgis-assistant` crate). Requests go out off the frame: on a
//! thread on the desktop, through `Services::http` (the browser's fetch) on the web.

use egui::{Align, Layout, RichText, Ui, vec2};
use rustgis_assistant::{Agent, Config, Entry, HttpRequest, HttpResponse, PRESETS, Step, chatgpt, provider};

use crate::theme::{Tokens, regular, semibold};
use crate::{RustGisApp, Slot, icons};

type Reply = Slot<Result<HttpResponse, rustgis_assistant::Error>>;

/// Example requests offered in an empty conversation.
const SUGGESTIONS: &[&str] = &[
    "Which 5 countries have the largest GDP?",
    "Zoom to Tokyo",
    "Label the cities with their names",
    "Buffer the cities by 200 km",
    "Switch to a dark basemap",
];

#[derive(Default)]
pub struct AssistantState {
    agent: Option<Agent>,
    pub input: String,
    /// A message to send on the next frame (`ui.pane.assistant {"ask": …}`).
    pub queued: Option<String>,
    /// The request out: its generation and where its response lands.
    inflight: Option<(u64, Reply)>,
    /// The settings form is open.
    pub settings: bool,
    show_key: bool,
    /// Keys are saved with the preferences only when the user says so.
    remember_key: bool,
    models: Vec<String>,
    catalog: Vec<provider::Model>,
    /// The catalog is fetched once per provider/account selection.
    catalog_pending: bool,
    models_slot: Option<Reply>,
    models_error: Option<rustgis_assistant::Error>,
    /// Earlier messages, for Up/Down in the composer.
    history: Vec<String>,
    history_pos: Option<usize>,
    /// "Allow for this conversation" in the approval card.
    always: bool,
    loaded: bool,
    /// Settings changed and not yet saved: they are saved once no settings field is being typed in.
    unsaved: bool,
    /// A ChatGPT sign-in under way in the browser (desktop only).
    #[cfg(not(target_arch = "wasm32"))]
    login: Option<chatgpt::Login>,
    /// The ChatGPT account signed in, for the settings.
    account: Option<chatgpt::Account>,
    login_error: Option<rustgis_assistant::Error>,
    /// A sign-out's revocation at OpenAI, under way (desktop only).
    #[cfg(not(target_arch = "wasm32"))]
    signout: Option<Slot<Result<(), rustgis_assistant::Error>>>,
    /// The first sign-in's confirmation that the ChatGPT plan is in use, until dismissed.
    welcome: bool,
    /// The saved ChatGPT registrations, read once (the file is locked across processes) and
    /// again after a sign-in, sign-out or selection.
    #[cfg(not(target_arch = "wasm32"))]
    accounts: Option<Vec<chatgpt::Credentials>>,
    /// The last sign-in added a second registration for an account already saved.
    #[cfg(not(target_arch = "wasm32"))]
    duplicate: bool,
}

/// The settings fields typed into (saved when none has focus).
const FIELDS: [&str; 3] = ["assistant_model", "assistant_url", "assistant_key"];

fn typing(ctx: &egui::Context) -> bool {
    ctx.memory(|m| FIELDS.iter().any(|f| m.has_focus(egui::Id::new(f))))
}

fn pref(app: &RustGisApp, key: &str) -> Option<String> {
    app.services.load_pref.as_ref().and_then(|l| l(key)).filter(|v| !v.is_empty())
}

fn save_pref(app: &RustGisApp, key: &str, value: &str) {
    if let Some(s) = &app.services.save_pref {
        s(key, value);
    }
}

/// A provider's settings: its defaults and the environment, then what the user saved.
fn load_config(app: &RustGisApp, provider: &str) -> Config {
    let mut c = Config::from_env(provider);
    if let Some(m) = pref(app, &format!("assistant_model_{provider}")) {
        c.model = m;
    }
    if let Some(u) = pref(app, &format!("assistant_url_{provider}")).filter(|_| !c.login()) {
        c.base_url = u;
    }
    if let Some(k) = pref(app, &format!("assistant_key_{provider}")).filter(|_| !c.login()) {
        c.key = k;
    }
    c
}

fn ensure_loaded(app: &mut RustGisApp) {
    if app.assistant.loaded {
        return;
    }
    app.assistant.loaded = true;
    app.assistant.remember_key = pref(app, "assistant_remember_key").as_deref() == Some("on");
    let provider = pref(app, "assistant_provider")
        .or_else(|| std::env::var("RUSTGIS_ASSISTANT_PROVIDER").ok())
        .filter(|p| provider::preset(p).is_some())
        // The first provider with a key in the environment (a ChatGPT sign-in is chosen, not assumed).
        .or_else(|| PRESETS.iter().find(|p| !p.login && !Config::from_env(p.id).key.is_empty()).map(|p| p.id.to_string()))
        .unwrap_or_else(|| "anthropic".into());
    let mut cfg = load_config(app, &provider);
    if let Ok(m) = std::env::var("RUSTGIS_ASSISTANT_MODEL")
        && !m.trim().is_empty()
        && pref(app, &format!("assistant_model_{provider}")).is_none()
    {
        cfg.model = m.trim().to_string();
    }
    app.assistant.settings = cfg.missing().is_some();
    if cfg.login() && !cfg.key.is_empty() {
        app.assistant.account = signed_in_account();
    }
    app.assistant.catalog_pending = cfg.login() && !cfg.key.is_empty();
    app.assistant.agent = Some(Agent::new(cfg, &app.session));
}

/// The ChatGPT account signed in on this computer, as its verified id token named it.
fn signed_in_account() -> Option<chatgpt::Account> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        chatgpt::account()
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

fn save_config(app: &RustGisApp, c: &Config) {
    save_pref(app, "assistant_provider", &c.provider);
    save_pref(app, &format!("assistant_model_{}", c.provider), c.model.trim());
    save_pref(app, &format!("assistant_url_{}", c.provider), c.base_url.trim());
    // A key from the environment stays there; a sign-in's tokens live in their own file.
    let from_env = Config::from_env(&c.provider).key;
    let keep = app.assistant.remember_key && c.key.trim() != from_env && !c.login();
    save_pref(app, &format!("assistant_key_{}", c.provider), if keep { c.key.trim() } else { "" });
}

/// Sends a request off the frame. A signed-in account's requests renew its tokens as needed.
fn dispatch(app: &mut RustGisApp, req: HttpRequest, login: Option<chatgpt::Binding>, ctx: &egui::Context) -> Reply {
    let slot: Reply = Slot::default();
    let fail = |slot: &Reply, e: String| {
        if let Ok(mut s) = slot.lock() {
            *s = Some(Err(e.into()));
        }
    };
    if login.is_some() && !chatgpt::api_url(&req.url) {
        fail(&slot, tl!("A ChatGPT sign-in can only be used with the OpenAI API.").into());
        return slot;
    }
    if !provider::allowed_url(&req.url) {
        fail(
            &slot,
            crate::i18n::fmt("{url}: only HTTPS addresses, or plain HTTP on this computer or network, are allowed", &[("url", &req.url)]),
        );
        return slot;
    }
    if let Some(http) = &app.services.http
        && login.is_none()
    {
        http(req, slot.clone());
        return slot;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (out, ctx) = (slot.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = match &login {
                Some(account) => chatgpt::send(&req, account),
                None => rustgis_assistant::http::send(&req).map_err(Into::into),
            };
            if let Ok(mut s) = out.lock() {
                *s = Some(r);
            }
            ctx.request_repaint();
        });
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (req, ctx, login);
        fail(&slot, "this build can't reach AI providers".into());
    }
    slot
}

/// Asks the provider for its models.
fn list_models(app: &mut RustGisApp, cfg: &Config, ctx: &egui::Context) {
    app.assistant.catalog_pending = false;
    let req = provider::models_request(cfg);
    app.assistant.models_slot = Some(dispatch(app, req, cfg.login().then(|| cfg.binding()), ctx));
}

fn apply_config(app: &mut RustGisApp, cfg: Config) {
    if let Some(agent) = app.assistant.agent.as_mut() {
        if agent.config.provider != cfg.provider
            || (agent.config.login() && (agent.config.binding() != cfg.binding() || agent.config.key != cfg.key))
        {
            agent.stop();
            app.assistant.inflight = None;
        }
        agent.config = cfg;
    }
}

/// A ChatGPT account signed in: the assistant uses its plan from now on.
#[cfg(not(target_arch = "wasm32"))]
fn signed_in(app: &mut RustGisApp, c: &chatgpt::Credentials, ctx: &egui::Context) {
    app.assistant.account = Some(c.account());
    app.assistant.login_error = None;
    let saved = chatgpt::accounts().unwrap_or_default();
    // "Use another account" with an account already saved registers RustGIS for it again.
    app.assistant.duplicate = saved.iter().any(|a| a.subject == c.subject && a.client_id != c.client_id);
    app.assistant.accounts = Some(saved);
    // The first sign-in says that the plan is now used (once).
    app.assistant.welcome = c.plan_allowed() && pref(app, "assistant_chatgpt_welcomed").as_deref() != Some("on");
    let Some(mut cfg) = app.assistant.agent.as_ref().map(|a| a.config.clone()) else { return };
    if !cfg.login() {
        return;
    }
    cfg.sign_in(c);
    app.assistant.models.clear();
    app.assistant.catalog.clear();
    app.assistant.models_error = None;
    if c.plan_allowed() {
        list_models(app, &cfg, ctx);
    } else {
        app.assistant.models_slot = None;
        app.assistant.catalog_pending = false;
        app.assistant.login_error = Some(rustgis_assistant::Error::new(rustgis_assistant::Recovery::Consent, ""));
    }
    apply_config(app, cfg);
    app.assistant.unsaved = true;
}

/// After a failed request: when the sign-in on this computer no longer has tokens (they expired
/// or were revoked), the settings say so and offer to sign in again.
fn sync_sign_in(app: &mut RustGisApp) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(a) = app.assistant.agent.as_mut()
        && a.config.login()
        && !a.config.key.is_empty()
        && chatgpt::load().is_none()
    {
        a.config.key.clear();
        app.assistant.account = None;
        app.assistant.welcome = false;
        app.assistant.settings = true;
        app.assistant.accounts = None;
    }
    #[cfg(target_arch = "wasm32")]
    let _ = app;
}

fn take(slot: &Reply) -> Option<Result<HttpResponse, rustgis_assistant::Error>> {
    slot.lock().ok().and_then(|mut s| s.take())
}

/// Carries out what the agent asked for.
fn follow(app: &mut RustGisApp, step: Step, ctx: &egui::Context) {
    app.map.invalidate();
    if let Step::Send(req) = step {
        let generation = app.assistant.agent.as_ref().map_or(0, Agent::generation);
        let login = app.assistant.agent.as_ref().filter(|a| a.config.login()).map(|a| a.config.binding());
        let slot = dispatch(app, req, login, ctx);
        app.assistant.inflight = Some((generation, slot));
    }
}

/// Runs `f` on the agent and the session; errors of the commands it ran show in the transcript,
/// not as notices too.
fn with_agent(app: &mut RustGisApp, f: impl FnOnce(&mut Agent, &mut rustgis_engine::Session) -> Step) -> Option<Step> {
    let mut agent = app.assistant.agent.take()?;
    let before = app.session.notices.len();
    let step = f(&mut agent, &mut app.session);
    let mut i = 0;
    app.session.notices.retain(|n| {
        i += 1;
        i <= before || !n.error
    });
    app.assistant.agent = Some(agent);
    Some(step)
}

/// Per-frame: responses that arrived, a message queued by a command, and settings to save.
pub fn poll(app: &mut RustGisApp, ctx: &egui::Context) {
    if app.assistant.catalog_pending
        && let Some(cfg) = app.assistant.agent.as_ref().map(|a| a.config.clone())
    {
        list_models(app, &cfg, ctx);
    }
    if app.assistant.unsaved && !typing(ctx) {
        app.assistant.unsaved = false;
        save_pref(app, "assistant_remember_key", if app.assistant.remember_key { "on" } else { "off" });
        if let Some(cfg) = app.assistant.agent.as_ref().map(|a| a.config.clone()) {
            save_config(app, &cfg);
        }
    }
    if let Some(text) = app.assistant.queued.take() {
        ensure_loaded(app);
        if app.assistant.agent.as_ref().is_some_and(Agent::busy) {
            app.session.notify(tl!("The assistant is busy; stop it or wait for it to finish."), true);
        } else {
            send(app, text, ctx);
        }
    }
    if let Some((generation, slot)) = &app.assistant.inflight
        && let Some(r) = take(slot)
    {
        let generation = *generation;
        app.assistant.inflight = None;
        let failed = r.is_err();
        if let Some(step) = with_agent(app, |a, s| a.receive_result(generation, r, s)) {
            follow(app, step, ctx);
        }
        if failed {
            sync_sign_in(app);
        }
    }
    if let Some(slot) = &app.assistant.models_slot
        && let Some(r) = take(slot)
    {
        app.assistant.models_slot = None;
        let api = app.assistant.agent.as_ref().map_or(rustgis_assistant::Api::OpenAi, |a| a.config.api());
        let models = r.and_then(|r| {
            if api == rustgis_assistant::Api::Responses {
                let catalog = provider::parse_catalog(&r)?;
                if catalog.is_empty() {
                    return Err("No models are available for this account.".into());
                }
                if let Some(a) = app.assistant.agent.as_mut() {
                    provider::use_catalog(&mut a.config, &catalog, false);
                    app.assistant.unsaved = true;
                }
                let ids = catalog.iter().map(|m| m.id.clone()).collect();
                app.assistant.catalog = catalog;
                Ok(ids)
            } else {
                provider::parse_models(api, &r).map_err(Into::into)
            }
        });
        match models {
            Ok(m) => {
                // A signed-in account's catalog says which models it may use: the first is
                // the one to suggest when the chosen one isn't offered.
                if let Some(a) = app.assistant.agent.as_mut()
                    && a.config.login()
                    && let Some(first) = m.first()
                    && !m.contains(&a.config.model)
                {
                    a.config.model = first.clone();
                    app.assistant.unsaved = true;
                }
                app.assistant.models = m;
                app.assistant.models_error = None;
            }
            Err(e) => {
                app.assistant.models_error = Some(e);
                sync_sign_in(app);
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(r) = app.assistant.login.as_ref().and_then(chatgpt::Login::poll) {
        app.assistant.login = None;
        match r {
            Ok(c) => signed_in(app, &c, ctx),
            Err(e) => app.assistant.login_error = Some(e),
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(r) = app.assistant.signout.as_ref().and_then(|s| s.lock().ok().and_then(|mut s| s.take())) {
        app.assistant.signout = None;
        if r.is_err() {
            app.assistant.login_error = Some(
                tl!("Signed out on this computer, but OpenAI didn't confirm it. You can disconnect RustGIS in ChatGPT's settings.").into(),
            );
        }
    }
}

fn send(app: &mut RustGisApp, text: String, ctx: &egui::Context) {
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }
    app.assistant.history.retain(|h| *h != text);
    app.assistant.history.push(text.clone());
    app.assistant.history_pos = None;
    app.assistant.always = false;
    let session = &app.session;
    let Some(step) = app.assistant.agent.as_mut().map(|a| a.ask(&text, session)) else { return };
    app.assistant.input.clear();
    follow(app, step, ctx);
}

pub fn show(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    ensure_loaded(app);
    let ctx = ui.ctx().clone();
    if app.assistant.catalog_pending
        && let Some(cfg) = app.assistant.agent.as_ref().map(|a| a.config.clone())
    {
        list_models(app, &cfg, &ctx);
    }
    let busy = app.assistant.agent.as_ref().is_some_and(Agent::busy);
    // The model and settings, at the top.
    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.horizontal(|ui| {
            let (label, model) = app
                .assistant
                .agent
                .as_ref()
                .map(|a| (provider::preset(&a.config.provider).map_or("", |p| p.label), a.config.model.clone()))
                .unwrap_or_default();
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::hover());
            icons::paint(ui.painter(), r, "assistant", t.text_dim, t.accent);
            let model_label = egui::Label::new(RichText::new(format!("{} · {}", tl!(label), model)).small().color(t.text_dim))
                .truncate()
                .sense(egui::Sense::click());
            // Room for the two buttons; a long model name is cut short.
            let w = (ui.available_width() - 2.0 * (26.0 + ui.spacing().item_spacing.x)).max(0.0);
            if ui
                .allocate_ui(vec2(w, 16.0), |ui| ui.add(model_label))
                .inner
                .on_hover_text(tl!("The model answering; click to change it"))
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                app.assistant.settings = !app.assistant.settings;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if crate::widgets::icon_button(ui, "settings", tl!("Assistant settings")).clicked() {
                    app.assistant.settings = !app.assistant.settings;
                }
                let has_chat = app.assistant.agent.as_ref().is_some_and(|a| !a.transcript.is_empty());
                if has_chat && crate::widgets::icon_button(ui, "delete", tl!("New conversation")).clicked() {
                    if let Some(a) = app.assistant.agent.as_mut() {
                        a.clear();
                    }
                    app.assistant.inflight = None;
                }
            });
        });
    });
    let plan = app.assistant.agent.as_ref().is_some_and(|a| a.config.login());
    if plan {
        composer_panel(app, ui, t, busy, &ctx);
    }
    if app.assistant.settings {
        let mut draw = |ui: &mut Ui| {
            egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 4)).show(ui, |ui| settings(app, ui, t));
        };
        if plan {
            egui::ScrollArea::vertical()
                .id_salt("assistant_plan_settings")
                .auto_shrink([false, true])
                .min_scrolled_height(0.0)
                .max_height((ui.available_height() - 8.0).max(0.0))
                .show(ui, draw);
        } else {
            draw(ui);
        }
        ui.separator();
    }
    if !plan {
        composer_panel(app, ui, t, busy, &ctx);
    }
    // The conversation in between.
    let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true);
    if plan {
        scroll = scroll.min_scrolled_height(0.0);
    }
    scroll.show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
            // Text in a row (a list item, a quote, the user's message) wraps too, instead of widening the pane.
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            transcript(app, ui, t, &ctx);
        });
    });
}

fn composer_panel(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, busy: bool, ctx: &egui::Context) {
    egui::Panel::bottom("assistant_composer")
        .frame(egui::Frame::NONE.fill(t.pane).inner_margin(egui::Margin::symmetric(10, 8)))
        .show(ui, |ui| composer(app, ui, t, busy, ctx));
}

fn settings(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let Some(mut cfg) = app.assistant.agent.as_ref().map(|a| a.config.clone()) else { return };
    let before = (cfg.clone(), app.assistant.remember_key);
    ui.label(RichText::new(tl!("AI provider")).font(semibold(12.5)));
    egui::Grid::new("assistant_settings").num_columns(2).spacing(vec2(8.0, 6.0)).show(ui, |ui| {
        ui.label(tl!("Provider"));
        let current = provider::preset(&cfg.provider).map_or("", |p| p.label);
        egui::ComboBox::from_id_salt("assistant_provider").selected_text(tl!(current)).width(180.0).show_ui(ui, |ui| {
            for p in PRESETS {
                // Signing in needs the browser and a server on this computer: the desktop app.
                if p.login && cfg!(target_arch = "wasm32") {
                    continue;
                }
                if ui.selectable_label(cfg.provider == p.id, tl!(p.label)).clicked() && cfg.provider != p.id {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        app.assistant.login = None;
                    }
                    cfg = load_config(app, p.id);
                    app.assistant.models_slot = None;
                    app.assistant.models.clear();
                    app.assistant.catalog.clear();
                    app.assistant.models_error = None;
                    app.assistant.login_error = None;
                    if cfg.login() && !cfg.key.is_empty() {
                        app.assistant.account = signed_in_account();
                        let ctx = ui.ctx().clone();
                        list_models(app, &cfg, &ctx);
                    }
                }
            }
        });
        ui.end_row();
        if cfg.login() {
            ui.label(tl!("Account"));
            ui.vertical(|ui| account_row(app, ui, t, &mut cfg));
            ui.end_row();
        }
        ui.label(tl!("Model"));
        let signed_out = cfg.login() && cfg.key.is_empty();
        ui.add_enabled_ui(!signed_out, |ui| {
            ui.horizontal(|ui| {
                if signed_out {
                    ui.label(tl!("Sign in to choose a model."));
                    return;
                }
                ui.add(
                    egui::TextEdit::singleline(&mut cfg.model).id(egui::Id::new(FIELDS[0])).desired_width(150.0).hint_text(tl!("model id")),
                );
                if !app.assistant.models.is_empty() {
                    egui::ComboBox::from_id_salt("assistant_models").selected_text("").width(24.0).height(320.0).show_ui(ui, |ui| {
                        for m in &app.assistant.models {
                            if ui
                                .selectable_label(
                                    cfg.model == *m,
                                    app.assistant.catalog.iter().find(|c| c.id == *m).map_or(m.as_str(), |c| c.name.as_str()),
                                )
                                .clicked()
                            {
                                cfg.model = m.clone();
                            }
                        }
                    });
                }
                let loading = app.assistant.models_slot.is_some();
                if ui
                    .add_enabled(!loading, egui::Button::new(if loading { "…" } else { "↻" }))
                    .on_hover_text(tl!("List the provider's models"))
                    .clicked()
                {
                    let ctx = ui.ctx().clone();
                    list_models(app, &cfg, &ctx);
                }
            });
        });
        ui.end_row();
        let preset = provider::preset(&cfg.provider);
        if !cfg.login() {
            ui.label(tl!("Address"));
            ui.add(egui::TextEdit::singleline(&mut cfg.base_url).id(egui::Id::new(FIELDS[1])).desired_width(200.0));
            ui.end_row();
            ui.label(tl!("API key"));
            ui.horizontal(|ui| {
                let hint = if preset.is_some_and(|p| p.keyless) { tl!("not needed") } else { tl!("paste your key") };
                ui.add(
                    egui::TextEdit::singleline(&mut cfg.key)
                        .id(egui::Id::new(FIELDS[2]))
                        .password(!app.assistant.show_key)
                        .desired_width(150.0)
                        .hint_text(hint),
                );
                let eye = if app.assistant.show_key { tl!("Hide") } else { tl!("Show") };
                if ui.small_button(eye).clicked() {
                    app.assistant.show_key = !app.assistant.show_key;
                }
            });
        }
        ui.end_row();
    });
    let preset = provider::preset(&cfg.provider);
    let mut notes = Vec::new();
    if cfg.login() {
        notes.push(
            tl!("The sign-in is kept in the app's settings folder, for your user account only, and never in a project file.").to_string(),
        );
        notes.push(tl!("Requests go to the OpenAI API and use your ChatGPT plan.").to_string());
    } else {
        ui.checkbox(&mut app.assistant.remember_key, tl!("Remember the key on this computer"));
        if let Some(p) = preset.filter(|p| !p.key_env.is_empty()) {
            notes.push(crate::i18n::fmt("On the desktop, the key can also come from {vars}.", &[("vars", &p.key_env.join(", "))]));
        }
        notes.push(
            if app.assistant.remember_key {
                tl!("Saved keys are kept unencrypted in the app's preferences (in the browser's storage on the web).")
            } else {
                tl!("The key is kept for this session only.")
            }
            .to_string(),
        );
    }
    notes.push(
        tl!("Requests send your message and the map's layer names, fields and the rows the assistant reads to the provider.").to_string(),
    );
    for n in notes {
        ui.label(RichText::new(n).small().color(t.text_dim));
    }
    if let Some(p) = preset.filter(|p| !p.key_url.is_empty() && !p.login) {
        ui.hyperlink_to(RichText::new(if p.keyless { tl!("Get it") } else { tl!("Get a key") }).small(), p.key_url);
    }
    if let Some(e) = &app.assistant.models_error {
        show_error(ui, e, t);
    }
    if cfg.login() && !cfg.key.is_empty() {
        plan_notice(app, ui, t);
    }
    if let Some(e) = &app.assistant.login_error {
        show_error(ui, e, t);
    }
    if let Some(why) = cfg.missing() {
        ui.label(RichText::new(tl!(why)).small().color(t.amber));
    }
    if cfg.login() {
        provider::use_catalog(&mut cfg, &app.assistant.catalog, true);
    }
    if (cfg.clone(), app.assistant.remember_key) != before {
        app.assistant.unsaved = true;
        apply_config(app, cfg);
    }
}

/// The settings' account row of a sign-in provider: who is signed in and Sign out, or
/// Continue with ChatGPT (the browser).
#[cfg(not(target_arch = "wasm32"))]
fn account_row(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, cfg: &mut Config) {
    if let Some(login) = &app.assistant.login {
        let (reopen, cancel) = waiting_for_browser(ui, t, &login.url);
        if reopen {
            app.ui.open_url = Some(login.browser_url.clone());
        }
        if cancel {
            app.assistant.login = None;
        }
        return;
    }
    let saved = app.assistant.accounts.get_or_insert_with(|| chatgpt::accounts().unwrap_or_default()).clone();
    let active = saved.iter().find(|c| c.client_id == cfg.client_id && c.subject == cfg.account);
    let label = active.map(chatgpt::Credentials::label).unwrap_or_else(|| tl!("Choose an account").into());
    let picker = egui::ComboBox::from_id_salt("assistant_account").selected_text(label).width(180.0).truncate().show_ui(ui, |ui| {
        for account in &saved {
            let selected = active.as_ref().is_some_and(|c| c.client_id == account.client_id && c.subject == account.subject);
            // Selecting the current row also recovers a window with an outdated sign-in.
            if ui.selectable_label(selected, account.label()).clicked() {
                app.assistant.accounts = None;
                app.assistant.duplicate = false;
                match chatgpt::select(&account.client_id, &account.subject) {
                    Ok(c) => {
                        cfg.sign_in(&c);
                        if !c.plan_allowed() {
                            cfg.key.clear();
                        }
                        app.assistant.account = Some(c.account());
                        app.assistant.models.clear();
                        app.assistant.catalog.clear();
                        app.assistant.models_slot = None;
                        app.assistant.models_error = None;
                        if !cfg.key.is_empty() {
                            list_models(app, cfg, ui.ctx());
                        }
                    }
                    Err(e) => app.assistant.login_error = Some(e.auth_context()),
                }
            }
        }
    });
    // Opening the list reads it again: another window or the command line may have added one.
    if picker.response.clicked() {
        app.assistant.accounts = None;
    }
    let another = ui.small_button(tl!("Use another account")).clicked();
    if another {
        start_login(app, cfg, true);
        return;
    }
    if !cfg.key.is_empty() {
        let who = app.assistant.account.as_ref().map(|a| a.email.clone()).filter(|e| !e.is_empty());
        ui.label(RichText::new(who.unwrap_or_else(|| tl!("signed in").to_string())).small());
        if ui.small_button(tl!("Sign out")).clicked() {
            match chatgpt::sign_out_selected(&cfg.binding()) {
                Ok(revocation) => {
                    // Revoking the session at OpenAI is network I/O: off the frame.
                    if let Some(r) = revocation {
                        let slot: Slot<Result<(), rustgis_assistant::Error>> = Slot::default();
                        let (out, ctx) = (slot.clone(), ui.ctx().clone());
                        std::thread::spawn(move || {
                            let result = chatgpt::revoke(&r);
                            if let Ok(mut s) = out.lock() {
                                *s = Some(result);
                            }
                            ctx.request_repaint();
                        });
                        app.assistant.signout = Some(slot);
                    }
                }
                Err(e) => {
                    app.assistant.login_error = Some(e.auth_context());
                    return;
                }
            }
            app.assistant.models_slot = None;
            cfg.key.clear();
            app.assistant.account = None;
            app.assistant.models.clear();
            app.assistant.catalog.clear();
            app.assistant.welcome = false;
            app.assistant.accounts = None;
        }
        if app.assistant.duplicate {
            ui.label(
                RichText::new(tl!("This ChatGPT account was already saved. Next time, choose it in the list instead of adding it again."))
                    .small()
                    .color(t.text_dim),
            );
        }
        return;
    }
    if ui.button(tl!("Continue with ChatGPT")).clicked() {
        start_login(app, cfg, false);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn waiting_for_browser(ui: &mut Ui, t: &Tokens, url: &str) -> (bool, bool) {
    ui.spinner();
    ui.label(RichText::new(tl!("Waiting for the browser…")).small().color(t.text_dim));
    let reopen = ui.small_button(tl!("Open browser again")).clicked();
    if ui.small_button(tl!("Copy sign-in link")).clicked() {
        ui.ctx().copy_text(url.to_string());
    }
    (reopen, ui.small_button(tl!("Cancel")).clicked())
}

#[cfg(not(target_arch = "wasm32"))]
fn start_login(app: &mut RustGisApp, cfg: &mut Config, another: bool) {
    if !another && !cfg.client_id.is_empty() {
        match chatgpt::select(&cfg.client_id, &cfg.account) {
            Ok(c) => cfg.sign_in(&c),
            Err(e) => {
                app.assistant.login_error = Some(e.auth_context());
                return;
            }
        }
    }
    match chatgpt::login(another) {
        Ok(login) => {
            app.ui.open_url = Some(login.browser_url.clone());
            app.assistant.login = Some(login);
            app.assistant.login_error = None;
        }
        Err(e) => app.assistant.login_error = Some(e),
    }
}

/// Recovery text is translated; server diagnostics retain their original wording.
fn show_error(ui: &mut Ui, error: &rustgis_assistant::Error, t: &Tokens) {
    ui.label(RichText::new(tl!(error.message())).small().color(t.red));
    if error.recovery == rustgis_assistant::Recovery::Usage
        && crate::widgets::primary_button(ui, tl!("Weekly usage"), true, vec2(160.0, 26.0)).clicked()
    {
        ui.ctx().open_url(egui::OpenUrl::new_tab(chatgpt::USAGE_URL));
    }
    if error.status.is_some() || error.recovery != rustgis_assistant::Recovery::Other {
        ui.collapsing(tl!("Diagnostics"), |ui| {
            ui.label(&error.detail);
            if let Some(status) = error.status {
                ui.monospace(format!("HTTP {status}"));
            }
            for diagnostic in [&error.code, &error.param, &error.request_id].into_iter().flatten() {
                ui.monospace(diagnostic);
            }
        });
    }
}

/// While a ChatGPT plan is in use: saying so, with a link to its usage settings, and after the
/// first sign-in, a confirmation to dismiss.
fn plan_notice(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    if app.assistant.welcome {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(RichText::new(tl!("You're using your ChatGPT plan")).font(semibold(12.5)));
            ui.label(
                RichText::new(tl!("Eligible requests in RustGIS use your ChatGPT plan. You can manage usage in ChatGPT settings."))
                    .small()
                    .color(t.text_dim),
            );
            if ui.button(tl!("Got it")).clicked() {
                app.assistant.welcome = false;
                save_pref(app, "assistant_chatgpt_welcomed", "on");
            }
        });
    }
    plan_line(ui, t);
}

/// "Using ChatGPT plan · Weekly usage", near the composer and in the settings.
fn plan_line(ui: &mut Ui, t: &Tokens) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(tl!("Using ChatGPT plan")).small().color(t.text_dim));
        ui.hyperlink_to(RichText::new(tl!("Weekly usage")).small(), chatgpt::USAGE_URL);
    });
}

#[cfg(target_arch = "wasm32")]
fn account_row(_app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, _cfg: &mut Config) {
    ui.label(RichText::new(tl!("Signing in with ChatGPT works in the desktop app.")).small().color(t.text_dim));
}

fn composer(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, busy: bool, ctx: &egui::Context) {
    // An action waiting for approval comes first.
    let pending = app.assistant.agent.as_ref().and_then(|a| a.pending(&app.session));
    if let Some((what, why, can_always)) = pending {
        egui::Frame::NONE.fill(t.amber.gamma_multiply(0.15)).corner_radius(6).inner_margin(8).stroke(egui::Stroke::new(1.0, t.amber)).show(
            ui,
            |ui| {
                ui.label(RichText::new(tl!("Allow this action?")).font(semibold(12.5)));
                ui.label(RichText::new(crate::i18n::fmt("The assistant wants to run this, and {why}:", &[("why", tl!(&why))])).small());
                ui.label(RichText::new(what).monospace().small());
                // Plugin commands, scripts and replacing the project are asked about every time.
                if can_always {
                    ui.checkbox(&mut app.assistant.always, tl!("Allow such actions for this conversation"));
                } else {
                    app.assistant.always = false;
                }
                ui.horizontal(|ui| {
                    let allow = crate::widgets::primary_button(ui, tl!("Allow"), true, vec2(70.0, 26.0)).clicked();
                    let deny = ui.button(tl!("Decline")).clicked();
                    if allow || deny {
                        let always = app.assistant.always;
                        if let Some(step) = with_agent(app, |a, s| a.decide(allow, always, s)) {
                            follow(app, step, ctx);
                        }
                    }
                });
            },
        );
        ui.add_space(6.0);
    }
    if app.assistant.agent.as_ref().is_some_and(|a| a.config.login() && !a.config.key.is_empty()) {
        plan_line(ui, t);
    }
    let id = egui::Id::new("assistant_input");
    // Enter sends; Shift+Enter starts a new line.
    let focused = ui.memory(|m| m.has_focus(id));
    let mut submit = false;
    if focused && !busy {
        submit = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        // Up and Down step through earlier messages while the box holds one of them (or nothing).
        let shown = app.assistant.history_pos.and_then(|p| app.assistant.history.get(p)).is_some_and(|h| *h == app.assistant.input);
        if app.assistant.input.is_empty() || shown {
            let n = app.assistant.history.len();
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) && n > 0 {
                let p = app.assistant.history_pos.map_or(n - 1, |p| p.saturating_sub(1));
                app.assistant.history_pos = Some(p);
                app.assistant.input = app.assistant.history.get(p).cloned().unwrap_or_default();
            } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown))
                && let Some(p) = app.assistant.history_pos
            {
                app.assistant.history_pos = (p + 1 < n).then_some(p + 1);
                app.assistant.input = app.assistant.history_pos.and_then(|p| app.assistant.history.get(p)).cloned().unwrap_or_default();
            }
        }
    }
    ui.add(
        egui::TextEdit::multiline(&mut app.assistant.input)
            .id(id)
            .desired_rows(2)
            .desired_width(f32::INFINITY)
            .font(regular(13.0))
            .hint_text(tl!("Ask about the map, or tell it what to do…")),
    );
    ui.horizontal(|ui| {
        // Room for the button; the hint gives way in long languages.
        let w = (ui.available_width() - 84.0).max(0.0);
        ui.allocate_ui(vec2(w, 20.0), |ui| {
            ui.add(egui::Label::new(RichText::new(tl!("Enter to send, Shift+Enter for a new line")).small().color(t.text_dim)).truncate());
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if busy {
                if ui.button(tl!("Stop")).clicked() {
                    if let Some(a) = app.assistant.agent.as_mut() {
                        a.stop();
                    }
                    app.assistant.inflight = None;
                }
            } else {
                let can = !app.assistant.input.trim().is_empty();
                if crate::widgets::primary_button(ui, tl!("Send"), can, vec2(64.0, 26.0)).clicked() {
                    submit = true;
                }
            }
        });
    });
    if submit && !busy {
        let text = app.assistant.input.clone();
        send(app, text, ctx);
        ui.memory_mut(|m| m.request_focus(id));
    }
}

fn transcript(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, ctx: &egui::Context) {
    let Some(agent) = app.assistant.agent.as_ref() else { return };
    if agent.transcript.is_empty() {
        ui.label(RichText::new(tl!("Ask a question about your data, or describe what to do with the map. The assistant runs RustGIS commands for you; every change can be undone.")).color(t.text_dim));
        ui.add_space(8.0);
        let mut chosen = None;
        for s in SUGGESTIONS {
            let b = egui::Button::new(RichText::new(tl!(s)).small()).corner_radius(12);
            if ui.add(b).clicked() {
                chosen = Some(tl!(s).to_string());
            }
        }
        if let Some(text) = chosen {
            send(app, text, ctx);
        }
        return;
    }
    for e in &agent.transcript {
        match e {
            Entry::User(text) => {
                ui.add_space(6.0);
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    egui::Frame::NONE.fill(t.selection).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                        ui.set_max_width(ui.available_width() * 0.85);
                        ui.add(egui::Label::new(RichText::new(text).color(t.text)).halign(Align::LEFT));
                    });
                });
                ui.add_space(4.0);
            }
            Entry::Assistant(text) => {
                markdown(ui, text, t);
                ui.add_space(4.0);
            }
            Entry::Tool { name, summary, result, error } => {
                let (mark, col) = if *error { ("✗", t.red) } else { ("✓", t.green) };
                let head = format!("{mark} {}", summary.chars().take(90).collect::<String>());
                // A header never wraps: it is cut to the room there is. Opened, it shows the whole call.
                let room = (ui.available_width() - ui.spacing().indent - ui.spacing().button_padding.x).max(0.0);
                let head = egui::WidgetText::from(RichText::new(head).monospace().small().color(col)).into_galley(
                    ui,
                    Some(egui::TextWrapMode::Truncate),
                    room,
                    egui::TextStyle::Button,
                );
                egui::CollapsingHeader::new(head).id_salt(("assistant_tool", name, summary.as_str(), result.len())).show(ui, |ui| {
                    ui.label(RichText::new(summary).monospace().small());
                    ui.label(RichText::new(result.chars().take(2000).collect::<String>()).monospace().small().color(t.text_dim));
                });
            }
            Entry::Error(e) => {
                show_error(ui, e, t);
            }
            Entry::Note(n) => {
                ui.label(RichText::new(tl!(n)).small().italics().color(t.text_dim));
            }
        }
    }
    if agent.waiting() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new(tl!("Thinking…")).small().color(t.text_dim));
        });
    }
}

// ---------------------------------------------------------------------------------------------
// A small Markdown renderer: headings, lists, quotes, code blocks, tables (as text), **bold**,
// *italic*, `code` and links (shown as text). Model output is text, never markup to run.

fn markdown(ui: &mut Ui, text: &str, t: &Tokens) {
    let mut code: Option<Vec<&str>> = None;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            match code.take() {
                Some(lines) => code_block(ui, &lines.join("\n"), t),
                None => code = Some(Vec::new()),
            }
            continue;
        }
        if let Some(lines) = code.as_mut() {
            lines.push(line);
            continue;
        }
        let trimmed = line.trim_start();
        // A few levels at most: a reply indented further would push its text out of the pane.
        let indent = ((line.len() - trimmed.len()) as f32 * 4.0).min(48.0);
        if trimmed.is_empty() {
            ui.add_space(4.0);
        } else if let Some(h) = ["### ", "## ", "# "].iter().find_map(|p| trimmed.strip_prefix(p)) {
            ui.add_space(2.0);
            ui.label(inline(ui, h, t, 14.0, true));
        } else if let Some(item) = ["- ", "* ", "+ "].iter().find_map(|p| trimmed.strip_prefix(p)) {
            bullet(ui, indent, "•", item, t);
        } else if let Some((num, item)) = numbered(trimmed) {
            bullet(ui, indent, &format!("{num}."), item, t);
        } else if let Some(q) = trimmed.strip_prefix("> ") {
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.label(RichText::new("▎").color(t.border_strong));
                ui.label(inline(ui, q, t, 13.0, false));
            });
        } else if trimmed.starts_with('|') {
            // Table rows as monospace text; the divider row is dropped.
            if !trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
                let cells: Vec<&str> = trimmed.trim_matches('|').split('|').map(str::trim).collect();
                ui.label(RichText::new(cells.join("  │  ")).monospace().small());
            }
        } else {
            ui.label(inline(ui, trimmed, t, 13.0, false));
        }
    }
    if let Some(lines) = code {
        code_block(ui, &lines.join("\n"), t);
    }
}

fn numbered(s: &str) -> Option<(&str, &str)> {
    let (num, rest) = s.split_once(". ")?;
    (!num.is_empty() && num.len() <= 3 && num.chars().all(|c| c.is_ascii_digit())).then_some((num, rest))
}

fn bullet(ui: &mut Ui, indent: f32, mark: &str, item: &str, t: &Tokens) {
    ui.horizontal_top(|ui| {
        ui.add_space(4.0 + indent);
        ui.label(RichText::new(mark).color(t.text_dim));
        ui.label(inline(ui, item, t, 13.0, false));
    });
}

fn code_block(ui: &mut Ui, code: &str, t: &Tokens) {
    egui::Frame::NONE.fill(t.input).stroke(egui::Stroke::new(1.0, t.border)).corner_radius(4).inner_margin(6).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(code).monospace().small());
    });
}

/// A line with **bold**, *italic*, `code` and [links](url) (as their text) laid out.
fn inline(ui: &Ui, text: &str, t: &Tokens, size: f32, heading: bool) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let mut job = LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    let base = |bold: bool, italic: bool, code: bool| {
        let font = if code {
            egui::FontId::monospace(size - 1.0)
        } else if bold || heading {
            semibold(size)
        } else {
            regular(size)
        };
        TextFormat {
            font_id: font,
            color: if code { t.accent_text } else { t.text },
            italics: italic,
            background: if code { t.input } else { egui::Color32::TRANSPARENT },
            ..Default::default()
        }
    };
    let (mut bold, mut italic) = (false, false);
    let mut buf = String::new();
    let flush = |job: &mut LayoutJob, buf: &mut String, bold: bool, italic: bool| {
        if !buf.is_empty() {
            job.append(buf, 0.0, base(bold, italic, false));
            buf.clear();
        }
    };
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if let Some(r) = rest.strip_prefix("**").or_else(|| rest.strip_prefix("__")) {
            flush(&mut job, &mut buf, bold, italic);
            bold = !bold;
            rest = r;
        } else if c == '`'
            && let Some(end) = rest.get(1..).and_then(|r| r.find('`'))
        {
            flush(&mut job, &mut buf, bold, italic);
            job.append(rest.get(1..1 + end).unwrap_or(""), 0.0, base(bold, italic, true));
            rest = rest.get(end + 2..).unwrap_or("");
        } else if c == '*' && rest.get(1..).is_some_and(|r| !r.starts_with(' ')) {
            flush(&mut job, &mut buf, bold, italic);
            italic = !italic;
            rest = rest.get(1..).unwrap_or("");
        } else if c == '['
            && let Some(close) = rest.find("](")
            && let Some(end) = rest.get(close..).and_then(|r| r.find(')'))
        {
            buf.push_str(rest.get(1..close).unwrap_or(""));
            rest = rest.get(close + end + 1..).unwrap_or("");
        } else {
            buf.push(c);
            rest = rest.get(c.len_utf8()..).unwrap_or("");
        }
    }
    flush(&mut job, &mut buf, bold, italic);
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_app() -> RustGisApp {
        let mut app = RustGisApp::with_sample();
        let mut cfg = Config::from_env("chatgpt");
        cfg.key.clear();
        cfg.account.clear();
        cfg.client_id.clear();
        app.assistant.agent = Some(Agent::new(cfg, &app.session));
        app.assistant.loaded = true;
        app.run("ui.pane.assistant", serde_json::Value::Null).unwrap();
        app.assistant.settings = true;
        app
    }

    #[test]
    fn plan_settings_order_and_signed_out_model() {
        use egui_kittest::kittest::{NodeT, Queryable};
        let mut h = egui_kittest::Harness::builder().with_size(vec2(800.0, 600.0)).build_ui_state(
            |ui, app: &mut RustGisApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            plan_app(),
        );
        h.run();
        let provider = h.get_by_label("Provider").rect().min.y;
        let account = h.get_by_label("Account").rect().min.y;
        let model = h.get_by_label("Model").rect().min.y;
        assert!(provider < account && account < model);
        assert!(h.query_by_label("Address").is_none());
        assert!(h.get_by_label("Sign in to choose a model.").accesskit_node().is_disabled());
        let mut cfg = Config::from_env("custom");
        cfg.model = "local-model".into();
        h.state_mut().assistant.agent.as_mut().unwrap().config = cfg;
        h.run();
        assert!(h.query_by_label("Address").is_some());
        assert!(h.query_by_label("API key").is_some());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn waiting_controls_reopen_copy_and_cancel() {
        use egui_kittest::kittest::Queryable;
        let url = "https://auth.openai.com/synthetic-sign-in";
        let mut h = egui_kittest::Harness::builder().with_size(vec2(260.0, 220.0)).build_ui_state(
            |ui, actions: &mut (bool, bool)| {
                let (reopen, cancel) = waiting_for_browser(ui, &Tokens::light(), url);
                actions.0 |= reopen;
                actions.1 |= cancel;
            },
            (false, false),
        );
        h.step();
        for label in ["Waiting for the browser…", "Open browser again", "Copy sign-in link", "Cancel"] {
            let rect = h.get_by_label(label).rect();
            assert!(rect.max.x <= 260.0 && rect.max.y <= 220.0, "{label}: {rect:?}");
        }
        h.get_by_label("Open browser again").click();
        h.step();
        assert_eq!(*h.state(), (true, false));
        h.get_by_label("Copy sign-in link").click();
        h.step();
        assert!(h.output().platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::CopyText(text) if text == url)));
        h.get_by_label("Cancel").click();
        h.step();
        assert_eq!(*h.state(), (true, true));
    }

    #[test]
    fn tall_plan_settings_keep_transcript_above_composer() {
        use egui_kittest::kittest::Queryable;
        for (width, height) in [(960.0, 700.0), (800.0, 600.0)] {
            let mut app = plan_app();
            app.assistant.login_error = Some(rustgis_assistant::Error::new(rustgis_assistant::Recovery::Auth, ""));
            let mut h = egui_kittest::Harness::builder().with_size(vec2(width, height)).build_ui_state(
                |ui, app: &mut RustGisApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                app,
            );
            h.run();
            assert!(h.get_by_label("Send").rect().max.y <= height - 26.0);
            let input_top = h.get_by_role(egui::accesskit::Role::MultilineTextInput).rect().min.y;
            for shape in &h.output().shapes {
                if let egui::epaint::Shape::Text(text) = &shape.shape
                    && SUGGESTIONS.contains(&text.galley.job.text.as_str())
                {
                    assert!(shape.clip_rect.max.y <= input_top, "suggestions overlap the composer");
                }
            }
        }
    }

    /// Runs the test `name` again in a child process whose settings folder is a fresh temporary
    /// one, so ChatGPT credentials and lock files never touch the developer's own (the
    /// environment of this process is left alone). Gives whether this is the child, which runs
    /// the test's body.
    #[cfg(not(target_arch = "wasm32"))]
    fn in_isolated_settings(name: &str) -> bool {
        if std::env::var_os("RUSTGIS_TEST_ISOLATED").is_some() {
            return true;
        }
        let short = name.rsplit("::").next().unwrap_or(name);
        let dir = std::env::temp_dir().join(format!("rustgis-ui-{short}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name])
            .env("RUSTGIS_TEST_ISOLATED", "1")
            .env("XDG_CONFIG_HOME", &dir)
            .env("APPDATA", &dir)
            .env("HOME", &dir)
            .env("RUSTGIS_LANGUAGE", "en")
            .output()
            .unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
        assert!(stdout.contains("1 passed"), "the isolated run didn't run {name}:\n{stdout}");
        false
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn saved_accounts_load_models_at_startup_and_on_provider_and_account_switch() {
        use egui_kittest::kittest::Queryable;
        if !in_isolated_settings("assistantpane::tests::saved_accounts_load_models_at_startup_and_on_provider_and_account_switch") {
            return;
        }
        let a = chatgpt::Credentials {
            client_id: "test-personal".into(),
            subject: "test-user".into(),
            email: "same@example.test".into(),
            // No expiry and no refresh token: a request fails at renewal, without network I/O.
            access_token: "synthetic-token".into(),
            scopes: vec![chatgpt::PLAN_SCOPE.into()],
            ..Default::default()
        };
        let b = chatgpt::Credentials { client_id: "test-workspace".into(), ..a.clone() };
        let path = chatgpt::auth_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::json!({"version":2, "active":{"client_id":a.client_id,"subject":a.subject},
            "accounts":[a.to_json(),b.to_json()]})
            .to_string(),
        )
        .unwrap();
        let mut app = RustGisApp::with_sample();
        app.services.load_pref = Some(Box::new(|key| (key == "assistant_provider").then(|| "chatgpt".into())));
        ensure_loaded(&mut app);
        assert!(app.assistant.catalog_pending);
        let ctx = egui::Context::default();
        poll(&mut app, &ctx);
        assert!(app.assistant.models_slot.is_some() || app.assistant.models_error.is_some());
        app.run("ui.pane.assistant", serde_json::Value::Null).unwrap();
        app.assistant.settings = true;
        // Draw settings without polling, so each click's catalog request remains observable.
        // A fixed number of frames, not `run`: each catalog request ends on a thread that asks for a
        // repaint, which can land mid-run (it did on Windows) and run past `run`'s step limit.
        let mut fonts = false;
        let mut h = egui_kittest::Harness::builder().with_size(vec2(400.0, 600.0)).build_ui_state(
            move |ui, app: &mut RustGisApp| {
                if !fonts {
                    crate::theme::install_fonts(ui.ctx(), &[], None);
                    fonts = true;
                    ui.ctx().request_repaint();
                    return;
                }
                settings(app, ui, &Tokens::light());
            },
            app,
        );
        h.run_steps(4);
        h.state_mut().assistant.models_slot = None;
        h.get_by_value("ChatGPT (sign in)").click();
        h.run_steps(4);
        h.get_by_label("Ollama (local)").click();
        h.run_steps(4);
        assert!(h.state().assistant.models_slot.is_none());
        h.get_by_value("Ollama (local)").click();
        h.run_steps(4);
        h.get_by_label("ChatGPT (sign in)").click();
        h.run_steps(4);
        assert!(h.state().assistant.models_slot.is_some());
        h.state_mut().assistant.models_slot = None;
        h.get_by_value(&a.label()).click();
        h.run_steps(4);
        h.get_by_label(&b.label()).click();
        h.run_steps(4);
        assert!(h.state().assistant.models_slot.is_some());
        assert_eq!(h.state().assistant.agent.as_ref().unwrap().config.client_id, b.client_id);
        assert_eq!(chatgpt::load_any().unwrap().client_id, b.client_id);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn sign_in_loads_catalog_and_applies_first_offered_model() {
        if !in_isolated_settings("assistantpane::tests::sign_in_loads_catalog_and_applies_first_offered_model") {
            return;
        }
        let mut app = plan_app();
        let c = chatgpt::Credentials {
            access_token: "synthetic-token".into(),
            client_id: "client".into(),
            subject: "user".into(),
            scopes: vec![chatgpt::PLAN_SCOPE.into()],
            ..Default::default()
        };
        let ctx = egui::Context::default();
        signed_in(&mut app, &c, &ctx);
        assert!(app.assistant.models_slot.is_some());
        let slot = Slot::default();
        *slot.lock().unwrap() = Some(Ok(HttpResponse { status: 200, request_id: None,
            body: serde_json::json!({"models":[{"slug":"offered", "display_name":"Offered model", "visibility":"list", "supports_reasoning":false}]}).to_string().into_bytes() }));
        app.assistant.models_slot = Some(slot);
        poll(&mut app, &ctx);
        let cfg = &app.assistant.agent.as_ref().unwrap().config;
        assert_eq!(cfg.model, "offered");
        assert_eq!(cfg.reasoning, Some(false));
        assert_eq!(app.assistant.catalog[0].name, "Offered model");
        signed_in(&mut app, &chatgpt::Credentials { scopes: vec![], ..c }, &ctx);
        assert!(app.assistant.models_slot.is_none());
        assert!(app.assistant.agent.as_ref().unwrap().config.key.is_empty());
        assert!(!app.assistant.welcome);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_plan_requests_cannot_bypass_auth_through_host_http() {
        if !in_isolated_settings("assistantpane::tests::native_plan_requests_cannot_bypass_auth_through_host_http") {
            return;
        }
        let mut app = plan_app();
        app.services.http = Some(Box::new(|_, _| panic!("the host hook must not receive plan credentials")));
        let request = HttpRequest { method: "GET", url: format!("{}/models", chatgpt::API), headers: vec![], body: None };
        let slot = dispatch(&mut app, request, Some(chatgpt::Binding::default()), &egui::Context::default());
        let start = std::time::Instant::now();
        loop {
            if let Some(result) = take(&slot) {
                assert_eq!(result.unwrap_err().recovery, rustgis_assistant::Recovery::SignIn);
                break;
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(2));
            std::thread::yield_now();
        }
    }

    #[test]
    fn usage_error_offers_weekly_usage() {
        use egui_kittest::kittest::Queryable;
        let error = rustgis_assistant::Error::new(rustgis_assistant::Recovery::Usage, "");
        let mut app = plan_app();
        app.assistant.settings = false;
        app.assistant.agent.as_mut().unwrap().transcript.push(Entry::Error(error));
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 860.0)).with_max_steps(64).build_ui_state(
            |ui, app: &mut RustGisApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
        h.run();
        assert!(h.query_by_label("Weekly usage").is_some());
        h.get_by_label("Weekly usage").click();
        h.step();
        assert!(
            h.output().platform_output.commands.iter().any(|c| {
                matches!(c, egui::OutputCommand::OpenUrl(url) if url.url == "https://chatgpt.com/settings/usage?tab=overview")
            })
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn late_sign_in_never_changes_another_providers_key() {
        if !in_isolated_settings("assistantpane::tests::late_sign_in_never_changes_another_providers_key") {
            return;
        }
        let mut app = RustGisApp::with_sample();
        let cfg = Config {
            provider: "custom".into(),
            base_url: "http://localhost:1234/v1".into(),
            model: "test".into(),
            key: "original-key".into(),
            account: String::new(),
            client_id: String::new(),
            session: String::new(),
            reasoning: None,
            tool_output_token_limit: rustgis_assistant::tools::DEFAULT_OUTPUT_TOKENS,
        };
        app.assistant.agent = Some(Agent::new(cfg.clone(), &app.session));
        let c = chatgpt::Credentials { access_token: "synthetic-chatgpt-token".into(), ..Default::default() };
        signed_in(&mut app, &c, &egui::Context::default());
        assert_eq!(app.assistant.agent.as_ref().unwrap().config, cfg);
        assert!(app.assistant.models_slot.is_none());
    }

    #[test]
    fn provider_change_stops_the_previous_request() {
        let mut app = RustGisApp::with_sample();
        let cfg = Config {
            provider: "custom".into(),
            base_url: "http://localhost:1234/v1".into(),
            model: "test".into(),
            key: String::new(),
            account: String::new(),
            client_id: String::new(),
            session: String::new(),
            reasoning: None,
            tool_output_token_limit: rustgis_assistant::tools::DEFAULT_OUTPUT_TOKENS,
        };
        let mut agent = Agent::new(cfg.clone(), &app.session);
        assert!(matches!(agent.ask("hello", &app.session), Step::Send(_)));
        let generation = agent.generation();
        app.assistant.agent = Some(agent);
        app.assistant.inflight = Some((generation, Slot::default()));
        apply_config(&mut app, Config { provider: "openai".into(), ..cfg });
        let agent = app.assistant.agent.as_ref().unwrap();
        assert!(!agent.busy());
        assert_ne!(agent.generation(), generation);
        assert!(app.assistant.inflight.is_none());
    }

    /// The right pane's width with `entries` in the conversation and `model` as the model's name.
    fn pane_width(entries: Vec<Entry>, model: &str) -> f32 {
        let h = chat(entries, model);
        right_pane(&h).width()
    }

    fn right_pane(h: &egui_kittest::Harness<'_, RustGisApp>) -> egui::Rect {
        egui::containers::panel::PanelState::load(&h.ctx, egui::Id::new("right_pane")).unwrap().outer_rect
    }

    /// The window with the assistant open and `entries` in the conversation.
    fn chat(entries: Vec<Entry>, model: &str) -> egui_kittest::Harness<'static, RustGisApp> {
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 860.0)).with_max_steps(64).build_ui_state(
            |ui, app: &mut RustGisApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            RustGisApp::with_sample(),
        );
        h.state_mut().run("ui.pane.assistant", serde_json::Value::Null).unwrap();
        for _ in 0..4 {
            h.step();
        }
        let app = h.state_mut();
        app.assistant.settings = false;
        let agent = app.assistant.agent.as_mut().unwrap();
        agent.config.model = model.into();
        agent.transcript = entries;
        for _ in 0..6 {
            h.step();
        }
        h
    }

    /// #307: a reply wider than the pane pushed its edge off the window, so the map covered the
    /// chat and the edge could not be dragged. The pane stays in the window and keeps resizing.
    #[test]
    fn wide_reply_keeps_the_pane_in_the_window_and_resizable() {
        let sentence = "The assistant ran the buffer tool on the cities layer and added the result to the map. ".repeat(4);
        let mut h = chat(vec![Entry::Assistant(format!("{}- {sentence}", " ".repeat(400)))], "m");
        for _ in 0..20 {
            h.step();
        }
        let before = right_pane(&h);
        let window = h.ctx.content_rect();
        assert!(before.max.x <= window.max.x + 0.5, "the pane's edge left the window: {before:?} in {window:?}");
        // Drag the pane's edge 150 points towards the map (grabbed just inside the pane, as the
        // map's own drag sense is on top at the boundary).
        let edge = egui::pos2(before.min.x + 2.0, before.center().y);
        h.hover_at(edge);
        h.step();
        h.drag_at(edge);
        h.step();
        for dx in [50.0, 100.0, 150.0] {
            h.hover_at(edge - vec2(dx, 0.0));
            h.step();
        }
        h.event(egui::Event::PointerButton {
            pos: edge - vec2(150.0, 0.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        for _ in 0..4 {
            h.step();
        }
        let after = right_pane(&h);
        assert!((after.min.x - (edge.x - 150.0)).abs() < 3.0, "the pane did not follow the drag: {before:?} to {after:?}");
        assert!(after.max.x <= window.max.x + 0.5, "the pane's edge left the window: {after:?} in {window:?}");
    }

    #[test]
    fn long_lines_wrap_instead_of_widening_the_pane() {
        let sentence = "The assistant ran the buffer tool on the cities layer and added the result to the map. ".repeat(4);
        let empty = pane_width(Vec::new(), "m");
        let mut widened = Vec::new();
        for (what, entry, model) in [
            ("a list item", Entry::Assistant(format!("- {sentence}")), "m"),
            ("a numbered item", Entry::Assistant(format!("1. {sentence}")), "m"),
            ("a quote", Entry::Assistant(format!("> {sentence}")), "m"),
            ("a word with no spaces", Entry::Assistant(format!("- {}", "x".repeat(400))), "m"),
            ("a deeply indented item", Entry::Assistant(format!("{}- {sentence}", " ".repeat(200))), "m"),
            ("the user's message", Entry::User(sentence.clone()), "m"),
            (
                "a tool call",
                Entry::Tool { name: "run_command".into(), summary: "gp.run ".repeat(20), result: sentence.clone(), error: false },
                "m",
            ),
            (
                "the model's name",
                Entry::Assistant("Done.".into()),
                "provider/a-very-long-model-name-with-many-parts-preview-2026-10-14-extended",
            ),
        ] {
            let w = pane_width(vec![entry], model);
            if (w - empty).abs() > 0.5 {
                widened.push(format!("{what}: {empty} to {w}"));
            }
        }
        assert!(widened.is_empty(), "the pane grew wider with {widened:?}");
    }
}
