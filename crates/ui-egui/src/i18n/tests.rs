use super::*;

#[test]
fn parse_and_escape() {
    let (m, e) = parse("# c\nOpen\tÖffnen\nA\\nB\tX\\nY\nbad line\n");
    assert_eq!(m.get("Open").map(String::as_str), Some("Öffnen"));
    assert_eq!(m.get("A\nB").map(String::as_str), Some("X\nY"));
    assert_eq!(e.len(), 1);
}

#[test]
fn every_catalog_parses_cleanly_and_keeps_placeholders() {
    for l in &LANGUAGES[1..] {
        let (m, errors) = parse(l.source);
        assert!(errors.is_empty(), "{}: {errors:?}", l.code);
        for (en, tr) in &m {
            let names = |s: &str| -> Vec<String> {
                let mut v: Vec<String> = s.split('{').skip(1).filter_map(|p| p.split_once('}').map(|x| x.0.to_string())).collect();
                v.sort();
                v
            };
            assert_eq!(names(en), names(tr), "{}: placeholders differ in {en:?}", l.code);
        }
    }
}

#[test]
fn lookup_fallback_and_tags() {
    assert_eq!(tr(Lang::EN, "Explore"), "Explore");
    assert_eq!(tr(Lang::EN, "no such string"), "no such string");
    assert_eq!(lang_from_tag("de_DE.UTF-8").map(Lang::code), Some("de"));
    assert_eq!(lang_from_tag("pt-BR").map(Lang::code), Some("pt"));
    assert_eq!(lang_from_tag("zh-Hans-CN").map(Lang::code), Some("zh"));
    assert_eq!(lang_from_tag("zh_CN.UTF-8").map(Lang::code), Some("zh"));
    assert_eq!(lang_from_tag("zh-TW"), None);
    assert_eq!(lang_from_tag("zh-Hant-HK"), None);
    assert_eq!(lang_from_tag("C").map(Lang::code), Some("en"));
    assert_eq!(lang_from_tag("xx-YY"), None);
    assert_eq!(first_supported(["xx", "fr-CA", "de"]).map(Lang::code), Some("fr"));
    assert_eq!(fmt("{n} of {total} selected", &[("n", "3"), ("total", "{n}")]), "3 of {n} selected");
}

#[test]
fn plugin_strings_use_the_plugins_translations() {
    use rustgis_engine::plugins;
    let pkg = plugins::Package::from_zip(&plugins::testing::package("i18n-fallback", &[])).unwrap();
    plugins::load(pkg, "test").unwrap();
    let de = Lang::from_code("de").unwrap();
    // Not in RustGIS's catalog: the plugin's `i18n` has it.
    assert_eq!(tr(de, "Hello"), "Hallo");
    assert_eq!(tr(Lang::EN, "Hello"), "Hello");
    assert_eq!(tr(Lang::from_code("fr").unwrap(), "Hello"), "Hello");
    // RustGIS's own catalog comes first.
    assert_eq!(tr(de, "Open"), de.0.catalog().get("Open").copied().unwrap_or("Open"));
    plugins::unload("i18n-fallback");
}

#[test]
fn chatgpt_recovery_and_controls_have_real_translations() {
    use rustgis_assistant::{Error, Recovery};
    let mut keys = vec![
        "Weekly usage",
        "Open browser again",
        "Copy sign-in link",
        "Sign in to choose a model.",
        "Choose an account",
        "Diagnostics",
        "No models are available for this account.",
        "The reply stopped before completion; no actions were run.",
    ];
    let errors: Vec<_> = [
        Recovery::SignIn,
        Recovery::AccountChanged,
        Recovery::Consent,
        Recovery::Timeout,
        Recovery::Cancelled,
        Recovery::Auth,
        Recovery::Configuration,
        Recovery::Unsupported,
        Recovery::Ineligible,
        Recovery::Usage,
        Recovery::Later,
    ]
    .into_iter()
    .map(|r| Error::new(r, ""))
    .collect();
    keys.extend(errors.iter().map(Error::message));
    for l in &LANGUAGES[1..] {
        let (catalog, _) = parse(l.source);
        for key in &keys {
            let translated = catalog.get(*key).unwrap_or_else(|| panic!("{}: missing {key}", l.code));
            assert_ne!(translated, key, "{}: untranslated {key}", l.code);
        }
    }
}
