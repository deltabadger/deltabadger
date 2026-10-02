mod common;
use deltabadger::web::i18n::{self, Arg};
use std::collections::BTreeMap;

fn args(call: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
    call["args"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

#[test]
fn every_recorded_translation_call_is_reproduced() {
    let calls = common::vectors()["i18n"]["calls"].as_array().unwrap().clone();
    assert!(calls.len() > 80);
    let mut failures = vec![];
    for call in &calls {
        let (locale, key) = (call["locale"].as_str().unwrap(), call["key"].as_str().unwrap());
        let owned = args(call);
        let given: Vec<(&str, Arg)> = owned.iter().map(|(name, value)| match value.as_i64() {
            Some(n) => (name.as_str(), Arg::Count(n)),
            None => (name.as_str(), Arg::Text(value.as_str().unwrap())),
        }).collect();
        let (view, text) = (i18n::t(locale, key, &given), i18n::text(locale, key, &given));
        if view != call["view"].as_str().unwrap() { failures.push(format!("t({locale}, {key}) = {view:?}, Rails {}", call["view"])); }
        if text != call["text"].as_str().unwrap() { failures.push(format!("text({locale}, {key}) = {text:?}, Rails {}", call["text"])); }
    }
    assert!(failures.is_empty(), "{} differ:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn escaping_is_erb_utils() {
    for pair in common::vectors()["i18n"]["escape"].as_array().unwrap() {
        assert_eq!(i18n::escape(pair[0].as_str().unwrap()), pair[1].as_str().unwrap());
    }
}

#[test]
fn already_safe_markup_is_not_escaped_in_an_html_key() {
    let out = i18n::t("en", "ads.dca_profit_html", &[("years", Arg::Html("<b>4</b>")), ("profit", Arg::Text("<1>")), ("sp500_diff", Arg::Count(3))]);
    assert!(out.contains("<b>4</b>&nbsp;years") && out.contains("&lt;1&gt;%") && out.contains("3%</b> more"), "{out}");
}

/// The whole table against Rails' own loader, live: the YAML is read by Psych in Rails and by yaml-rust2 in
/// build.rs, and this is what proves they agree on every key and every text. It is not a committed fixture
/// because that would freeze all 25,000 texts; a translation edit must not need a re-recording.
#[test]
fn the_embedded_table_is_exactly_what_rails_loads_from_config_locales() {
    let scratch = tempfile::tempdir().unwrap();
    let out = scratch.path().join("translations.json");
    common::rails(scratch.path(), "development", &["runner", "script/rust/translations.rb", out.to_str().unwrap()]);
    let rails: BTreeMap<String, String> = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let rust: BTreeMap<String, String> = i18n::all().iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    assert!(rails.len() > 20_000, "Rails loaded only {} texts", rails.len());
    let only_rails: Vec<&String> = rails.keys().filter(|k| !rust.contains_key(*k)).take(10).collect();
    let only_rust: Vec<&String> = rust.keys().filter(|k| !rails.contains_key(*k)).take(10).collect();
    let different: Vec<&String> = rails.iter().filter(|(k, v)| rust.get(*k).is_some_and(|r| r != *v)).map(|(k, _)| k).take(10).collect();
    assert!(only_rails.is_empty() && only_rust.is_empty() && different.is_empty(),
            "only in Rails: {only_rails:?}\nonly in Rust: {only_rust:?}\ndifferent text: {different:?}");
}

/// Listed divergence: Rails commits a write the engine cannot run; Rust answers 422 with this copy.
/// One key, English only; every other locale falls back to it, as Rails' fallbacks do.
#[test]
fn the_write_refused_copy_carries_the_reason_in_english_in_every_locale() {
    let reason = "bot 7 (scheduled): quote_amount_limited";
    let en = i18n::text("en", "engine.write_refused", &[("reason", Arg::Text(reason))]);
    assert_eq!(en, format!("This app can't run that yet: {reason}"));
    for locale in ["de", "pl", "ru"] {
        assert_eq!(i18n::text(locale, "engine.write_refused", &[("reason", Arg::Text(reason))]), en, "{locale}");
    }
}

/// A web start on a bot that is already working is refused, as Rails' API refuses it (409
/// `bot_already_running`); the page re-renders with this flash. Listed divergence: Rails' web controller restarts the bot.
#[test]
fn the_already_running_refusal_is_one_english_key_every_locale_falls_back_to() {
    let en = i18n::text("en", "engine.already_running", &[]);
    assert_eq!(en, "This bot is already running.");
    for locale in ["de", "pl", "ru"] {
        assert_eq!(i18n::text(locale, "engine.already_running", &[]), en, "{locale}");
    }
}
