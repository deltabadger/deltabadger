//! PLAN-B11 §10 Q6 / r5.3 (B11-8): no tracked path or file says rails or ruby (any case, as a substring), except what
//! ALLOWED names, each entry with its reason. An entry that allows nothing fails too, so the list only shrinks.
use regex::bytes::Regex;
use std::process::Command;

/// (path regex, content regex, reason). A hit is allowed when its path matches and a match of the content regex covers
/// it. WHOLE allows the file's path and every byte, unread (the recorded answers are 2.4 GB).
const ALLOWED: &[(&str, &str, &str)] = &[];

const WHOLE: &str = "WHOLE";

struct Hit { path: String, line: usize, text: String }

fn rules() -> Vec<(Regex, Regex, &'static str)> {
    ALLOWED.iter().map(|(p, c, why)| (Regex::new(&format!("^(?:{p})$")).unwrap(), Regex::new(c).unwrap(), *why)).collect()
}

/// Every hit no rule allows, and the reasons of the rules that allowed nothing.
fn scan(files: &[String], read: impl Fn(&str) -> Vec<u8>) -> (Vec<Hit>, Vec<&'static str>) {
    let word = Regex::new("(?i)rails|ruby").unwrap();
    let rules = rules();
    let mut used = vec![false; rules.len()];
    let mut hits = vec![];
    for path in files {
        let mine: Vec<usize> = (0..rules.len()).filter(|&i| rules[i].0.is_match(path.as_bytes())).collect();
        if let Some(&i) = mine.iter().find(|&&i| rules[i].1.as_str() == WHOLE) { used[i] = true; continue; }
        if word.is_match(path.as_bytes()) { hits.push(Hit { path: path.clone(), line: 0, text: "(the path)".into() }); }
        let bytes = read(path);
        let bytes = &bytes;
        let spans: Vec<(usize, Vec<(usize, usize)>)> = mine.iter()
            .map(|&i| (i, rules[i].1.find_iter(bytes).map(|m| (m.start(), m.end())).collect())).collect();
        for m in word.find_iter(bytes) {
            match spans.iter().find(|(_, s)| s.iter().any(|&(a, b)| a <= m.start() && m.end() <= b)) {
                Some(&(i, _)) => used[i] = true,
                None => {
                    let line = bytes[..m.start()].iter().filter(|&&b| b == b'\n').count() + 1;
                    let start = bytes[..m.start()].iter().rposition(|&b| b == b'\n').map_or(0, |p| p + 1);
                    let end = bytes[m.start()..].iter().position(|&b| b == b'\n').map_or(bytes.len(), |p| m.start() + p);
                    let text: String = String::from_utf8_lossy(&bytes[start..end]).chars().take(160).collect();
                    hits.push(Hit { path: path.clone(), line, text });
                }
            }
        }
    }
    (hits, rules.iter().zip(used).filter(|(_, u)| !u).map(|(r, _)| r.2).collect())
}

fn root() -> std::path::PathBuf { std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..") }

/// Every tracked path; a path deleted in the working tree but not yet from the index reads as empty.
fn tracked() -> Vec<String> {
    let root = root();
    let out = Command::new("git").arg("-C").arg(&root).args(["ls-files", "-z"]).output().expect("git runs (the suite runs in a checkout)");
    assert!(out.status.success(), "git ls-files: {}", String::from_utf8_lossy(&out.stderr));
    out.stdout.split(|&b| b == 0).filter(|p| !p.is_empty()).map(|p| String::from_utf8(p.to_vec()).unwrap()).collect()
}

fn read(path: &str) -> Vec<u8> { std::fs::read(root().join(path)).unwrap_or_default() }

fn report(hits: &[Hit]) -> String {
    hits.iter().map(|h| format!("{}:{}: {}", h.path, h.line, h.text)).collect::<Vec<_>>().join("\n")
}

#[test]
fn no_tracked_path_or_file_says_rails_or_ruby_outside_the_allowlist() {
    let files = tracked();
    assert!(files.len() > 30_000, "{} tracked files", files.len());
    let (hits, unused) = scan(&files, read);
    assert!(hits.is_empty(), "{} hit(s) outside the allowlist:\n{}", hits.len(), report(&hits));
    assert!(unused.is_empty(), "allowlist entries that allow nothing (delete them):\n{}", unused.join("\n"));
}

#[test]
fn a_planted_word_fails_the_scan() {
    let planted = |path: &str, text: &str| {
        let mut files = tracked();
        files.push(path.into());
        let (hits, _) = scan(&files, |p| if p == path { text.as_bytes().to_vec() } else { read(p) });
        hits.iter().any(|h| h.path == path)
    };
    assert!(planted("rust/src/planted.rs", "// as Rails does\n"), "a comment");
    assert!(planted("rust/src/planted.rs", "let s = \"RUBY\";\n"), "a literal, any case");
    assert!(planted("rust/src/rails_planted.rs", "\n"), "a path");
    assert!(planted("rust/tests/planted.rs", "let v = &case[\"rails\"];\n"), "an unlisted reader of a recorded key");
    assert!(planted("rust/tests/mail.rs", "let v = &case[\"rails\"];\n"), "a recorded key read beside mail_parity.rs, not in it");
    assert!(planted("rust/migrations/20991231000001_new.sql", "-- Rails\n"), "a new migration");
}

/// The (c2) readers read keys the recordings hold, so the entries can't outlive what they name.
#[test]
fn the_recorded_key_readers_name_keys_the_recordings_hold() {
    let has = |file: &str, key: &str| String::from_utf8_lossy(&read(file)).contains(key);
    assert!(has("rust/tests/recorded/mails/cases.json", "\"rails\":"));
    let scenario = std::fs::read_dir(root().join("rust/tests/recorded")).unwrap().flatten()
        .filter(|g| g.file_name().to_string_lossy().starts_with("decisions-grid"))
        .flat_map(|g| std::fs::read_dir(g.path()).unwrap().flatten())
        .any(|s| std::fs::read_to_string(s.path().join("scenario.json")).is_ok_and(|t| t.contains("\"rails_floor\"")));
    assert!(scenario, "a decisions-grid scenario.json holds rails_floor");
}
