mod common;
use deltabadger::web::i18n::{self, Arg};

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
        if view != call["view"].as_str().unwrap() { failures.push(format!("t({locale}, {key}) = {view:?}, 2.x {}", call["view"])); }
        if text != call["text"].as_str().unwrap() { failures.push(format!("text({locale}, {key}) = {text:?}, 2.x {}", call["text"])); }
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

// PLAN-B11 T7 (L-I18N): the whole table is no longer compared with 2.x's I18n: a locale edit is read by build.rs only
// (its Psych-boolean/null refusal stays), never frozen, since every translation edit would need a re-recording.

/// Listed divergence: 2.x commits a write the engine cannot run; Rust answers 422 with this copy.
/// One key, English only; every other locale falls back to it, as 2.x's fallbacks do.
#[test]
fn the_write_refused_copy_carries_the_reason_in_english_in_every_locale() {
    let reason = "bot 7 (scheduled): quote_amount_limited";
    let en = i18n::text("en", "engine.write_refused", &[("reason", Arg::Text(reason))]);
    assert_eq!(en, format!("This app can't run that yet: {reason}"));
    for locale in ["de", "pl", "ru"] {
        assert_eq!(i18n::text(locale, "engine.write_refused", &[("reason", Arg::Text(reason))]), en, "{locale}");
    }
}

/// A web start on a bot that is already working is refused, as 2.x's API refuses it (409
/// `bot_already_running`); the page re-renders with this flash, as 2.x's web controller does (422).
#[test]
fn the_already_running_refusal_is_one_english_key_every_locale_falls_back_to() {
    let en = i18n::text("en", "engine.already_running", &[]);
    assert_eq!(en, "This bot is already running.");
    for locale in ["de", "pl", "ru"] {
        assert_eq!(i18n::text(locale, "engine.already_running", &[]), en, "{locale}");
    }
}

/// The dotiw gem's unit names are in the gem, not in rust/locales, so the crate pins a copy
/// (web::i18n FROM_GEMS). The recorder reads them from 2.x; a gem update that rewords one fails here.
#[test]
fn the_pinned_unit_names_are_the_dotiw_gems() {
    let recorded = common::vectors()["bot_pages"]["dotiw"].as_object().unwrap().clone();
    assert!(recorded.len() > 100, "only {} unit names were recorded", recorded.len());
    for (full_key, text) in &recorded {
        let (locale, key) = full_key.split_once('.').unwrap();
        assert_eq!(i18n::text(locale, key, &[]), text.as_str().unwrap(), "{full_key}");
    }
}

/// PLAN-B11 §5: no text a user reads says "2.x". Every translation, every template with its `{# #}` comments taken out,
/// and every string literal of the web layer and of the eligibility refusals (which pages and API answers show) outside
/// their unit tests. The literals still saying it are named below, each with why it is not page text or who rewords it.
#[test]
fn no_page_text_says_v2() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let word = regex::Regex::new(r"(?i)\brails\b").unwrap();
    let mut found: Vec<String> = i18n::all().iter().filter(|(_, text)| word.is_match(text)).map(|(key, _)| format!("translation {key}")).collect();
    let comment = regex::Regex::new(r"(?s)\{#.*?#\}").unwrap();
    let mut sources = vec![root.join("src/engine/eligibility.rs")];
    let mut dirs = vec![root.join("templates"), root.join("src/web")];
    let mut templates = 0;
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() { dirs.push(path); continue; }
            match path.extension().and_then(|e| e.to_str()) {
                Some("rs") => sources.push(path),
                Some(_) if path.starts_with(root.join("templates")) => {
                    templates += 1;
                    let text = std::fs::read_to_string(&path).unwrap();
                    if word.is_match(&comment.replace_all(&text, "")) { found.push(format!("template {}", path.strip_prefix(root).unwrap().display())); }
                }
                _ => {}
            }
        }
    }
    assert!(templates > 100 && sources.len() > 50, "{templates} templates, {} sources", sources.len());
    // None since B11-9 (PLAN-B11 §10 Q6): "2.x" replaces the word in every literal.
    let literal = regex::Regex::new(r#""(?:[^"\\]|\\.)*""#).unwrap();
    let capital = regex::Regex::new(r"\bRails\b").unwrap();
    for path in sources {
        let text = std::fs::read_to_string(&path).unwrap();
        let code = text.split("#[cfg(test)]").next().unwrap();
        let name = path.strip_prefix(root).unwrap().to_str().unwrap().replace('\\', "/");
        for line in code.lines().filter(|l| !l.trim_start().starts_with("//")) {
            for m in literal.find_iter(line).map(|m| m.as_str()).filter(|m| capital.is_match(m)) {
                found.push(format!("{name}: {m}"));
            }
        }
    }
    assert!(found.is_empty(), "says 2.x: {found:#?}");
}
