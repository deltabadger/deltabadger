//! Delisted coins (plan r10 §3): every reader of a raw spelling in non-test code outside `sync/`, `venue/` and
//! `tracker/import/`, per occurrence, with its decision (tests/spelling_readers.txt). PR0 marks every existing reader
//! `legacy` and the new resolver's own lines `resolver`; PR1 moves each to its decision in the commit that changes it.
//! A new reader, a second occurrence of a listed unit, or a listed unit gone fails. A unit is a logical line of code: a
//! line, or the lines an open `(` or `[` or a trailing operator join, comments out and string contents in; every test
//! item (`#[cfg(test)]`, `#[test]`, `#[tokio::test]`) is out, wherever it sits in the file.
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
const NEW: [&str; 3] = ["tracker/labels.rs", "tracker/resolver.rs", "tracker/spellings.rs"];

/// (file, kind, normalised line) → occurrences.
type Hits = BTreeMap<(String, &'static str, String), usize>;

fn sql() -> Vec<regex::Regex> {
    [r"\b(base|quote|fee)_currency\b", r"\b(t|tickers)\.(base|quote)\b", r"\b(a|assets)\.symbol\b", r"\bsymbol\s*=\s*\?", r"(?s)historical_prices.*\basset\s*=\s*\?"]
        .iter().map(|p| regex::Regex::new(p).unwrap()).collect()
}

/// `text` with every comment blanked (newlines kept), and a mask of it in which string and char literals are blanked
/// too: the mask shows the code's structure, the clean text its strings.
fn lex(text: &str) -> (String, String) {
    let b: Vec<char> = text.chars().collect();
    let (mut clean, mut mask) = (String::with_capacity(b.len()), String::with_capacity(b.len()));
    let blank = |c: char| if c == '\n' { '\n' } else { ' ' };
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let next = b.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < b.len() && b[i] != '\n' { clean.push(' '); mask.push(' '); i += 1; }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0;
            while i < b.len() {
                if b[i] == '/' && b.get(i + 1) == Some(&'*') { depth += 1; clean.push_str("  "); mask.push_str("  "); i += 2; continue; }
                if b[i] == '*' && b.get(i + 1) == Some(&'/') { depth -= 1; clean.push_str("  "); mask.push_str("  "); i += 2; if depth == 0 { break; } continue; }
                clean.push(blank(b[i])); mask.push(blank(b[i])); i += 1;
            }
        } else if c == 'r' && (next == Some('"') || next == Some('#')) && !b.get(i.wrapping_sub(1)).is_some_and(|p| p.is_alphanumeric() || *p == '_') {
            // A raw string: r"…", r#"…"#.
            let mut j = i + 1;
            while b.get(j) == Some(&'#') { j += 1; }
            let hashes = j - i - 1;
            if b.get(j) != Some(&'"') { clean.push(c); mask.push(c); i += 1; continue; }
            let closes = |k: usize| b[k] == '"' && (1..=hashes).all(|h| b.get(k + h) == Some(&'#'));
            let mut end = j + 1;
            while end < b.len() && !closes(end) { end += 1; }
            for &ch in &b[i..=j] { clean.push(ch); mask.push(ch); }
            for &ch in &b[j + 1..end.min(b.len())] { clean.push(ch); mask.push(blank(ch)); }
            let stop = (end + 1 + hashes).min(b.len());
            for &ch in &b[end.min(b.len())..stop] { clean.push(ch); mask.push(ch); }
            i = stop;
        } else if c == '"' {
            clean.push(c); mask.push(c); i += 1;
            while i < b.len() && b[i] != '"' {
                if b[i] == '\\' && i + 1 < b.len() { clean.push(b[i]); clean.push(b[i + 1]); mask.push(' '); mask.push(blank(b[i + 1])); i += 2; continue; }
                clean.push(b[i]); mask.push(blank(b[i])); i += 1;
            }
            if i < b.len() { clean.push('"'); mask.push('"'); i += 1; }
        } else if c == '\'' && (b.get(i + 2) == Some(&'\'') || (next == Some('\\') && b.get(i + 3) == Some(&'\''))) {
            // A char literal ('{', '\''); a lifetime has no closing quote two or three places on.
            let n = if next == Some('\\') { 4 } else { 3 };
            for (k, &ch) in b[i..i + n].iter().enumerate() { clean.push(ch); mask.push(if k == 0 || k == n - 1 { '\'' } else { ' ' }); }
            i += n;
        } else { clean.push(c); mask.push(c); i += 1; }
    }
    (clean, mask)
}

/// The code of a file as logical units (a line, or the lines an open `(` or `[` or a trailing operator joins), comments
/// out, every test item out: one under `#[cfg(test)]`, `#[test]` or `#[tokio::test]`, attributes, signature and body.
/// Each unit: (clean text, mask).
fn units(text: &str) -> Vec<(String, String)> {
    let (clean, mask) = without_tests(lex(text));
    let (cl, ml): (Vec<&str>, Vec<&str>) = (clean.lines().collect(), mask.lines().collect());
    let mut out = vec![];
    let (mut open, mut unit): (Vec<char>, (String, String)) = (vec![], (String::new(), String::new()));
    for i in 0..ml.len() {
        unit.0.push_str(cl[i]); unit.0.push('\n');
        unit.1.push_str(ml[i]); unit.1.push('\n');
        for ch in ml[i].chars() {
            match ch {
                '(' | '[' | '{' => open.push(ch),
                ')' | ']' | '}' => { open.pop(); }
                _ => {}
            }
        }
        // A unit ends at a line's end outside a string, with no `(` or `[` open since the innermost `{`, no operator
        // trailing and none (nor a method call, `?` or `.`) leading the next code line: a block's lines are units of
        // their own, an expression is one unit.
        let t = ml[i].trim_end();
        let joined = ["==", "!=", "||", "&&", "|", ",", "=", "+"].iter().any(|op| t.ends_with(op));
        let next = ml[i + 1..].iter().map(|l| l.trim_start()).find(|l| !l.is_empty()).unwrap_or("");
        let continued = [".", "?", "==", "!=", "&&", "||", "|"].iter().any(|op| next.starts_with(op)) && !next.starts_with("..");
        let in_string = unit.1.matches('"').count() % 2 == 1;
        if !in_string && !joined && !continued && open.last().is_none_or(|c| *c == '{') {
            if !unit.1.trim().is_empty() { out.push(std::mem::take(&mut unit)); } else { unit = Default::default(); }
        }
    }
    if !unit.1.trim().is_empty() { out.push(unit); }
    out
}

/// `(clean, mask)` with every test item blanked where it stands (newlines kept): an item under `#[cfg(test)]`,
/// `#[test]` or `#[tokio::test]`, from its attributes to its `;` (a match arm's or a field's `,`) or the `}` closing its
/// first `{` (or a `}` closing the item around it). Code before or after it on the same lines stays.
fn without_tests((clean, mask): (String, String)) -> (String, String) {
    let (mut c, mut m): (Vec<char>, Vec<char>) = (clean.chars().collect(), mask.chars().collect());
    let text: String = m.iter().collect();
    let mut starts: Vec<usize> = ["#[cfg(test)]", "#[test]", "#[tokio::test"].iter()
        .flat_map(|a| text.match_indices(a).map(|(k, _)| text[..k].chars().count()).collect::<Vec<_>>()).collect();
    starts.sort();
    let mut done = 0;
    for start in starts {
        if start < done { continue; }
        let (mut k, mut braces, mut parens, mut attr) = (start, 0i64, 0i64, 0i64);
        while k < m.len() {
            let ch = m[k];
            k += 1;
            if attr > 0 || (ch == '#' && m.get(k) == Some(&'[')) {
                // Inside an attribute: its brackets only.
                match ch { '[' => attr += 1, ']' => attr -= 1, _ => {} }
                continue;
            }
            match ch {
                '{' => braces += 1,
                '}' => { braces -= 1; if braces <= 0 { break; } }
                '(' | '[' => parens += 1,
                ')' | ']' => parens -= 1,
                // A test-only match arm or field ends at its comma; an item at its `;`.
                ';' | ',' if braces == 0 && parens == 0 => break,
                _ => {}
            }
        }
        for (mc, cc) in m[start..k].iter_mut().zip(c[start..k].iter_mut()) { if *mc != '\n' { *mc = ' '; *cc = ' '; } }
        done = k;
    }
    (c.into_iter().collect(), m.into_iter().collect())
}

/// Every hit in `files` ((path under src/, text)).
fn scan(files: &[(String, String)]) -> Hits {
    let patterns = sql();
    let literal = regex::Regex::new(r#""([0-9]*[A-Z][A-Z0-9]*)""#).unwrap();
    let mut hits = Hits::new();
    for (path, text) in files {
        if SKIPPED.iter().any(|s| path.starts_with(s)) { continue; }
        let literals = LITERAL_DIRS.iter().any(|d| path.starts_with(d));
        for (clean, mask) in units(text) {
            let normal = clean.split_whitespace().collect::<Vec<_>>().join(" ");
            if patterns.iter().any(|p| p.is_match(&normal)) { *hits.entry((path.clone(), "sql", normal.clone())).or_default() += 1; }
            let t = mask.trim_start();
            let constant = ["const ", "pub const ", "pub(crate) const ", "pub(super) const ", "static ", "pub static "].iter().any(|k| t.starts_with(k)) && !t.contains("const fn");
            let used = constant || ["==", "!=", "matches!", "contains("].iter().any(|op| mask.contains(op));
            let named = literal.captures_iter(&clean).any(|c| (2..=7).contains(&c[1].len()));
            if literals && used && named { *hits.entry((path.clone(), "literal", normal)).or_default() += 1; }
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

/// The list for `hits`: an entry `previous` lists with the same (file, kind, unit) keeps its decision; a new one is
/// `legacy` (`resolver` in the resolver's own files).
fn bless(hits: &Hits, previous: &str) -> String {
    let kept = listed(previous);
    let mut text = String::from("# decision\tfile\tkind\tcount\tunit (normalised); written by SPELLING_READERS_BLESS=1, reviewed by hand\n");
    for ((file, kind, line), n) in hits {
        let fresh = if NEW.contains(&file.as_str()) { "resolver" } else { "legacy" };
        let decision = kept.get(&(file.clone(), kind.to_string(), line.clone())).map_or(fresh, |(d, _)| d.as_str());
        text.push_str(&format!("{decision}\t{file}\t{kind}\t{n}\t{line}\n"));
    }
    text
}

fn list_file() -> std::path::PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join(LIST) }

#[test]
fn every_spelling_reader_is_listed() {
    let hits = scan(&sources());
    if std::env::var_os("SPELLING_READERS_BLESS").is_some() {
        let previous = std::fs::read_to_string(list_file()).unwrap_or_default();
        std::fs::write(list_file(), bless(&hits, &previous)).unwrap();
    }
    let list = listed(&std::fs::read_to_string(list_file()).unwrap_or_default());
    let problems = problems(&hits, &list);
    assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
    assert!(hits.len() > 150, "the scan still finds the readers: {}", hits.len());
}

/// A second occurrence of a listed line in its file fails, as does a listed line that is gone.
#[test]
fn a_second_occurrence_in_a_listed_file_fails() {
    let files = vec![("tracker/x.rs".to_string(), "let s = \"SELECT base_currency FROM account_transactions\";\n".to_string())];
    let list = listed(&bless(&scan(&files), ""));
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
    assert_eq!(problems, ["unlisted literal reader in tax/new.rs: const SET: [&str; 2] = [ \"USDP\", \"DAI\", ];",
                          "unlisted literal reader in tax/new.rs: fn emt(s: &str) -> bool { s == \"EURC\" }"]);
    let elsewhere = vec![("engine/new.rs".to_string(), files[0].1.clone())];
    assert!(scan(&elsewhere).is_empty(), "literals are scanned in tax, the tracker and figures only");
}

/// Codex PR0 P2-13: what a line-based scan missed: a `matches!` whose literal is on the next line, a `historical_prices`
/// query whose `asset = ?` is on the next line of the string, and production code after a test module, while a standalone
/// test function is not production code.
#[test]
fn the_scan_reads_expressions_sql_strings_and_test_items_structurally() {
    let file = |path: &str, text: &str| vec![(path.to_string(), text.to_string())];
    let kinds = |hits: &Hits| hits.keys().map(|(_, k, _)| *k).collect::<Vec<_>>();
    let predicate = file("tax/dk.rs", "fn dk(s: &str) -> bool {\n    matches!(\n        s,\n        \"USDT\" | \"USDC\"\n    )\n}\n");
    assert_eq!(kinds(&scan(&predicate)), ["literal"], "{:?}", scan(&predicate));
    let query = file("tracker/q.rs", "fn q() -> &'static str {\n    \"SELECT price FROM historical_prices \\\n     WHERE asset = ?1\"\n}\n");
    assert_eq!(kinds(&scan(&query)), ["sql"], "{:?}", scan(&query));
    let after = file("tracker/a.rs", "#[cfg(test)]\nmod tests {\n    fn t() { let s = \"base_currency\"; }\n}\n\nfn later() { let s = \"SELECT base_currency FROM t\"; }\n");
    let hits = scan(&after);
    assert_eq!(hits.keys().map(|(_, _, l)| l.as_str()).collect::<Vec<_>>(), ["fn later() { let s = \"SELECT base_currency FROM t\"; }"], "{hits:?}");
    let standalone = file("tracker/b.rs", "#[test]\nfn t() {\n    let s = \"SELECT base_currency FROM t\";\n}\n\n#[cfg(test)] mod more { fn u() { let s = \"quote_currency\"; } }\n");
    assert!(scan(&standalone).is_empty(), "{:?}", scan(&standalone));
}

/// Codex PR0 r2 P2-12: an array whose `.contains` is on the next line is one expression, and a production item on the
/// same line after a test item's closing brace is production code.
#[test]
fn the_scan_joins_method_chains_and_keeps_code_after_a_test_item() {
    let file = |path: &str, text: &str| vec![(path.to_string(), text.to_string())];
    let chain = file("tax/c.rs", "fn c(s: &str) -> bool {\n    [\"USDT\", \"USDC\"]\n        .contains(&s)\n}\n");
    assert_eq!(scan(&chain).keys().map(|(_, k, _)| *k).collect::<Vec<_>>(), ["literal"], "{:?}", scan(&chain));
    let same_line = file("tracker/s.rs", "#[cfg(test)] mod t { fn a() {} } fn later() { let s = \"SELECT base_currency FROM t\"; }\n");
    assert_eq!(scan(&same_line).len(), 1, "{:?}", scan(&same_line));
}

/// Codex PR0 r3 P3-11: a test-only match arm or field ends at its comma; the production arm after it is scanned.
#[test]
fn a_test_only_arm_hides_no_production_reader() {
    let arm = vec![("tracker/m.rs".to_string(), "fn f(kind: i32, row: &Row) -> &str { match kind { #[cfg(test)] 0 => \"\", _ => &row.base_currency } }\n".to_string())];
    assert_eq!(scan(&arm).len(), 1, "{:?}", scan(&arm));
}

/// Codex PR0 r4 P3: blessing keeps the decision of every unchanged (file, kind, unit) entry; only a new one is `legacy`.
#[test]
fn bless_keeps_reviewed_decisions() {
    let files = vec![("tracker/x.rs".to_string(), "let a = \"SELECT base_currency FROM t\";\nlet b = \"SELECT quote_currency FROM t\";\n".to_string())];
    let before = bless(&scan(&files), "").replace("legacy\ttracker/x.rs\tsql\t1\tlet a", "label\ttracker/x.rs\tsql\t1\tlet a");
    let again = listed(&bless(&scan(&files), &before));
    let decision = |unit: &str| again.iter().find(|((_, _, l), _)| l.starts_with(unit)).map(|(_, (d, _))| d.clone()).unwrap();
    assert_eq!((decision("let a"), decision("let b")), ("label".to_string(), "legacy".to_string()));
}
