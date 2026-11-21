//! Delisted coins (plan r10 §3): every reader of a raw spelling in non-test code outside `sync/`, `venue/` and
//! `tracker/import/`, per occurrence, with its decision (tests/spelling_readers.txt). PR0 marks every existing reader
//! `legacy` and the new resolver's own lines `resolver`; PR1 moves each to its decision in the commit that changes it.
//! A new reader, a second occurrence of a listed line, or a listed line gone fails.
//!
//! Patterns: SQL `base_currency`, `quote_currency`, `fee_currency`, `t.base`/`tickers.base`, `t.quote`/`tickers.quote`,
//! `a.symbol`/`assets.symbol`, `symbol = ?`, and `historical_prices … asset = ?`; and, inside `tax/`, `tracker/`,
//! `web/tracker*`, `web/shell.rs` and `figures/`, a string literal of 2–7 upper-case letters and digits used in `==`,
//! `!=`, `matches!`, `contains` or a `const`.
//!
//! To rewrite the list after a reviewed change: `SPELLING_READERS_BLESS=1 cargo nextest run --test spelling_readers`.
use std::collections::BTreeMap;
use std::path::Path;

const LIST: &str = "tests/spelling_readers.txt";
const SKIPPED: [&str; 3] = ["sync/", "venue/", "tracker/import/"];
const LITERAL_DIRS: [&str; 5] = ["tax/", "tracker/", "web/tracker", "web/shell.rs", "figures/"];
const NEW: [&str; 2] = ["tracker/resolver.rs", "tracker/spellings.rs"];

/// (file, kind, normalised line) → occurrences.
type Hits = BTreeMap<(String, &'static str, String), usize>;

fn sql() -> Vec<regex::Regex> {
    [r"\b(base|quote|fee)_currency\b", r"\b(t|tickers)\.(base|quote)\b", r"\b(a|assets)\.symbol\b", r"\bsymbol\s*=\s*\?", r"historical_prices.*\basset\s*=\s*\?"]
        .iter().map(|p| regex::Regex::new(p).unwrap()).collect()
}

/// The code of a file: up to its test module, comment lines left out.
fn code(text: &str) -> Vec<&str> {
    let lines: Vec<&str> = text.lines().collect();
    let end = lines.windows(2).position(|w| w[0].trim() == "#[cfg(test)]" && w[1].trim_start().starts_with("mod ")).unwrap_or(lines.len());
    lines[..end].iter().copied().filter(|l| !l.trim_start().starts_with("//")).collect()
}

/// Every hit in `files` ((path under src/, text)).
fn scan(files: &[(String, String)]) -> Hits {
    let (patterns, literal) = (sql(), regex::Regex::new(r#""[A-Z0-9]{2,7}""#).unwrap());
    let upper = regex::Regex::new(r#""[0-9]*[A-Z][A-Z0-9]*""#).unwrap();
    let mut hits = Hits::new();
    for (path, text) in files {
        if SKIPPED.iter().any(|s| path.starts_with(s)) { continue; }
        let literals = LITERAL_DIRS.iter().any(|d| path.starts_with(d));
        let mut in_const = false;
        for line in code(text) {
            let t = line.trim();
            if ["const ", "pub const ", "pub(crate) const "].iter().any(|k| t.starts_with(k)) && !t.contains("const fn") { in_const = true; }
            let normal = t.split_whitespace().collect::<Vec<_>>().join(" ");
            for p in &patterns {
                if p.is_match(line) { *hits.entry((path.clone(), "sql", normal.clone())).or_default() += 1; break; }
            }
            let used = in_const || ["==", "!=", "matches!", "contains("].iter().any(|op| line.contains(op));
            if literals && used && literal.find_iter(line).any(|m| upper.is_match(m.as_str())) {
                *hits.entry((path.clone(), "literal", normal.clone())).or_default() += 1;
            }
            if in_const && t.ends_with(';') { in_const = false; }
        }
    }
    hits
}

fn sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() { walk(&p, root, out); } else if p.extension().is_some_and(|e| e == "rs") {
                out.push((p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"), std::fs::read_to_string(&p).unwrap()));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = vec![];
    walk(&root, &root, &mut out);
    out
}

/// The list: (file, kind, line) → (decision, count).
fn listed(text: &str) -> BTreeMap<(String, String, String), (String, usize)> {
    text.lines().filter(|l| !l.is_empty() && !l.starts_with('#')).map(|l| {
        let f: Vec<&str> = l.splitn(5, '\t').collect();
        assert_eq!(f.len(), 5, "a list line is decision, file, kind, count, line: {l}");
        ((f[1].to_string(), f[2].to_string(), f[4].to_string()), (f[0].to_string(), f[3].parse().unwrap()))
    }).collect()
}

const DECISIONS: [&str; 7] = ["legacy", "resolver", "label", "asset", "delete", "raw-ok", "n/a"];

/// Every mismatch between the hits and the list.
fn problems(hits: &Hits, list: &BTreeMap<(String, String, String), (String, usize)>) -> Vec<String> {
    let mut out = vec![];
    for ((file, kind, line), n) in hits {
        match list.get(&(file.clone(), kind.to_string(), line.clone())) {
            None => out.push(format!("unlisted {kind} reader in {file}: {line}")),
            Some((_, m)) if m != n => out.push(format!("{file}: {n} occurrences of a line listed {m} times: {line}")),
            Some((decision, _)) if !DECISIONS.contains(&decision.as_str()) => out.push(format!("{file}: unknown decision {decision}: {line}")),
            _ => {}
        }
    }
    for (file, kind, line) in list.keys() {
        if !hits.keys().any(|(f, k, l)| f == file && *k == kind && l == line) { out.push(format!("listed but gone from {file}: {line}")); }
    }
    out
}

fn bless(hits: &Hits) -> String {
    let mut text = String::from("# decision\tfile\tkind\tcount\tline (normalised); written by SPELLING_READERS_BLESS=1, reviewed by hand\n");
    for ((file, kind, line), n) in hits {
        let decision = if NEW.contains(&file.as_str()) { "resolver" } else { "legacy" };
        text.push_str(&format!("{decision}\t{file}\t{kind}\t{n}\t{line}\n"));
    }
    text
}

fn list_file() -> std::path::PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join(LIST) }

#[test]
fn every_spelling_reader_is_listed() {
    let hits = scan(&sources());
    if std::env::var_os("SPELLING_READERS_BLESS").is_some() { std::fs::write(list_file(), bless(&hits)).unwrap(); }
    let list = listed(&std::fs::read_to_string(list_file()).unwrap_or_default());
    let problems = problems(&hits, &list);
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
    assert!(hits.len() > 150, "the scan still finds the readers: {}", hits.len());
}

/// A second occurrence of a listed line in its file fails, as does a listed line that is gone.
#[test]
fn a_second_occurrence_in_a_listed_file_fails() {
    let files = vec![("tracker/x.rs".to_string(), "let s = \"SELECT base_currency FROM account_transactions\";\n".to_string())];
    let list = listed(&bless(&scan(&files)));
    assert!(problems(&scan(&files), &list).is_empty());
    let twice = vec![("tracker/x.rs".to_string(), format!("{0}{0}", files[0].1))];
    assert_eq!(problems(&scan(&twice), &list), ["tracker/x.rs: 2 occurrences of a line listed 1 times: let s = \"SELECT base_currency FROM account_transactions\";"]);
    assert_eq!(problems(&Hits::new(), &list).len(), 1, "gone");
    let skipped = vec![("sync/x.rs".to_string(), files[0].1.clone()), ("tracker/y.rs".to_string(), format!("#[cfg(test)]\nmod tests {{\n{}}}\n", files[0].1))];
    assert!(scan(&skipped).is_empty(), "sync/ and test modules are out of the scan");
}

/// A new literal-symbol predicate in tax or the tracker fails until it is listed; one in a const array too.
#[test]
fn a_new_literal_predicate_fails() {
    let files = vec![("tax/new.rs".to_string(), "fn emt(s: &str) -> bool { s == \"EURC\" }\nconst SET: [&str; 2] = [\n    \"USDP\", \"DAI\",\n];\nlet x = \"usd\" == y;\n".to_string())];
    let problems = problems(&scan(&files), &BTreeMap::new());
    assert_eq!(problems, ["unlisted literal reader in tax/new.rs: \"USDP\", \"DAI\",", "unlisted literal reader in tax/new.rs: fn emt(s: &str) -> bool { s == \"EURC\" }"]);
    let elsewhere = vec![("engine/new.rs".to_string(), files[0].1.clone())];
    assert!(scan(&elsewhere).is_empty(), "literals are scanned in tax, the tracker and figures only");
}
