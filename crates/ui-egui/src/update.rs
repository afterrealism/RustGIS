//! Help › Check for Updates: asks GitHub for the latest release and compares it with this build.
//! Nothing is sent until the user asks.

use egui::Align2;
use serde_json::Value;

use crate::{RustGisApp, Slot};

/// The latest release, as JSON naming its tag and page.
const LATEST: &str = "https://api.github.com/repos/opengeos/RustGIS/releases/latest";
/// Every release, for when the check can't reach GitHub.
const RELEASES: &str = "https://github.com/opengeos/RustGIS/releases";
/// The release JSON is a few kilobytes; anything far larger isn't it.
const MAX_BYTES: usize = 1024 * 1024;

/// The Check for Updates window.
#[derive(Default)]
pub struct UpdateState {
    pub open: bool,
    /// Start a check on the next frame (commands have no egui context to repaint with).
    start: bool,
    pending: Option<Slot<Result<Vec<u8>, String>>>,
    result: Option<Result<Release, String>>,
}

impl UpdateState {
    /// Opens the window and checks again.
    pub fn check(&mut self) {
        self.open = true;
        self.start = true;
        self.result = None;
    }
}

/// A published release: its version (the tag without its `v`) and its page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub url: String,
}

/// Reads GitHub's latest-release JSON.
pub fn parse_release(body: &[u8]) -> Result<Release, String> {
    let v: Value = serde_json::from_slice(body).map_err(|_| "GitHub's reply isn't a release".to_string())?;
    let tag =
        v.get("tag_name").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).ok_or("GitHub's reply names no release")?;
    // A tag that isn't a version can't be compared: saying "up to date" would be a guess.
    if version(tag).is_none() {
        return Err("GitHub's latest release has no version number".into());
    }
    let url = v.get("html_url").and_then(Value::as_str).filter(|u| u.starts_with("https://")).unwrap_or(RELEASES);
    Ok(Release { version: tag.trim_start_matches(['v', 'V']).to_string(), url: url.to_string() })
}

/// A pre-release identifier: numbers sort numerically and before words (`beta.2` < `beta.10`,
/// `1` < `alpha`).
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Ident {
    Num(u64),
    Word(String),
}

/// A version's dotted numbers (one to three; missing ones are 0) and its pre-release
/// identifiers, without build metadata; `None` when it isn't a version.
fn version(v: &str) -> Option<([u64; 3], Vec<Ident>)> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let v = v.split('+').next().unwrap_or(v);
    let (core, pre) = v.split_once('-').map_or((v, None), |(c, p)| (c, Some(p)));
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut n = [0u64; 3];
    for (slot, part) in n.iter_mut().zip(&parts) {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    let pre = match pre {
        None => Vec::new(),
        Some("") => return None,
        Some(p) => p.split('.').map(|i| i.parse().map_or_else(|_| Ident::Word(i.to_string()), Ident::Num)).collect(),
    };
    Some((n, pre))
}

/// Whether version `latest` is newer than `current`, in semantic-version order: the numbers
/// first, then a pre-release (`0.2.0-beta.1`) before its release, and pre-releases by their
/// identifiers. False when either isn't a version.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let (Some((a, a_pre)), Some((b, b_pre))) = (version(latest), version(current)) else { return false };
    if a != b {
        return a > b;
    }
    match (a_pre.is_empty(), b_pre.is_empty()) {
        (true, true) | (false, true) => false,
        (true, false) => true,
        (false, false) => a_pre > b_pre,
    }
}

/// Draws the window, starting and collecting the check.
pub fn show(app: &mut RustGisApp, ctx: &egui::Context) {
    if !app.update.open {
        return;
    }
    // A check asked for while one is running waits for that one rather than sending another.
    if std::mem::take(&mut app.update.start) && app.update.pending.is_none() && !cfg!(target_arch = "wasm32") {
        app.update.pending = Some(crate::pluginsui::fetch(app, LATEST, MAX_BYTES, Some(ctx.clone())));
    }
    if let Some(r) = app.update.pending.as_ref().and_then(crate::pluginsui::take) {
        app.update.pending = None;
        app.update.result = Some(r.and_then(|body| parse_release(&body)));
    }
    let current = env!("CARGO_PKG_VERSION");
    let mut open = true;
    let mut again = false;
    let mut visit: Option<String> = None;
    egui::Window::new(tl!("Check for Updates"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .pivot(Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.set_width(380.0);
            if cfg!(target_arch = "wasm32") {
                ui.label(tl!("The web app always runs the latest release: reload the page to get it."));
                return;
            }
            match &app.update.result {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tl!("Checking for updates…"));
                    });
                }
                Some(Ok(r)) if is_newer(&r.version, current) => {
                    ui.label(egui::RichText::new(crate::i18n::fmt("RustGIS {latest} is available.", &[("latest", &r.version)])).strong());
                    ui.label(crate::i18n::fmt("You have version {version}.", &[("version", current)]));
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(tl!("Installed with Homebrew, winget, the AUR or COPR? Update it there.")).small().weak());
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button(tl!("Open the Release Page")).clicked() {
                            visit = Some(r.url.clone());
                        }
                        if ui.button(tl!("Check Again")).clicked() {
                            again = true;
                        }
                    });
                }
                Some(Ok(_)) => {
                    ui.label(crate::i18n::fmt("RustGIS {version} is the latest version.", &[("version", current)]));
                    ui.add_space(6.0);
                    if ui.button(tl!("Check Again")).clicked() {
                        again = true;
                    }
                }
                Some(Err(e)) => {
                    ui.label(crate::i18n::fmt("Couldn't check for updates: {error}", &[("error", tl!(e.as_str()))]));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button(tl!("Open the Release Page")).clicked() {
                            visit = Some(RELEASES.to_string());
                        }
                        if ui.button(tl!("Check Again")).clicked() {
                            again = true;
                        }
                    });
                }
            }
        });
    if again {
        app.update.check();
    }
    if visit.is_some() {
        app.ui.open_url = visit;
    }
    if !open {
        app.update.open = false;
        app.update.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number_with_pre_releases_first() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v0.1.1", "0.1.0"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("0.2.0", "0.2.0-beta.1"));
        assert!(!is_newer("0.2.0-beta.1", "0.2.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(is_newer("0.2.0-beta.2", "0.2.0-beta.1"));
        assert!(is_newer("0.2.0-beta.10", "0.2.0-beta.2"));
        assert!(is_newer("0.2.0-rc.1", "0.2.0-beta.3"));
        assert!(is_newer("0.2.0-beta", "0.2.0-1"));
        assert!(!is_newer("0.2.0-beta.1", "0.2.0-beta.1"));
        assert!(!is_newer("0.1.0+build.5", "0.1.0"));
        assert!(!is_newer("junk", "0.1.0"));
        assert!(!is_newer("release-2.0.0", "0.1.0"));
        assert!(!is_newer("1.2.3.4", "0.1.0"));
        assert!(!is_newer("", ""));
    }

    #[test]
    fn release_json_is_read_and_checked() {
        let r = parse_release(br#"{"tag_name":"v0.2.0","html_url":"https://github.com/opengeos/RustGIS/releases/tag/v0.2.0"}"#).unwrap();
        assert_eq!(r.version, "0.2.0");
        assert!(r.url.ends_with("/v0.2.0"));
        // A page that isn't HTTPS falls back to the releases list.
        assert_eq!(parse_release(br#"{"tag_name":"0.2.0","html_url":"javascript:alert(1)"}"#).unwrap().url, RELEASES);
        assert!(parse_release(b"<html>rate limited</html>").is_err());
        assert!(parse_release(br#"{"message":"Not Found"}"#).is_err());
        assert!(parse_release(br#"{"tag_name":"  "}"#).is_err());
        // A tag that isn't a version is an error, not "up to date".
        assert!(parse_release(br#"{"tag_name":"release-2.0.0"}"#).is_err());
        assert_eq!(parse_release(br#"{"tag_name":"v1.0.0-rc.1"}"#).unwrap().version, "1.0.0-rc.1");
    }
}
