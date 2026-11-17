//! PLAN-B11 §10 Q6 / r5.3 (B11-8): no tracked path or file says rails or ruby (any case, as a substring), except what
//! ALLOWED names, each entry with its reason. An entry that allows nothing fails too, so the list only shrinks.
use regex::bytes::Regex;
use std::process::Command;

/// (path regex, content regex, reason). A hit is allowed when its path matches and a match of the content regex covers
/// it. WHOLE allows the file's path and every byte, unread (the recorded answers are 2.4 GB).
const ALLOWED: &[(&str, &str, &str)] = &[
    (r#"rust/migrations/(20261022000001|20261023000001|20261025000001|20261026000001|20261028000001|20261030000001|20261102000001|20261108000001|20261109000001|20261110000001|20261111000001|20261112000001)_[a-z0-9_]+\.sql"#,
     WHOLE,
     r#"(a) released migrations: applied SQL never changes (B1); B11-9's migration names old and new"#),
    (r#"rust/(src|tests|examples)/.*\.rs"#,
     r#"rust_rails_(?:attempt_acks|job_acks|unrecorded(?:_live|_offers|_resolutions|_requests|_notices)?|wakes|queue_done)\b|index_rust_rails_unrecorded_on_source|\brails_unrecorded_report\b|\brails_at\b|\brails_order_[a-z_]*|'rails'|\brails:|rails-job:|status Rails never writes"#,
     r#"(b) B11-9's stored names and values (PLAN-B11-9 §1.1, §6 list (b)); B11-9 PR2 empties this"#),
    (r#"rust/src/parity\.rs"#,
     r#"20261110000001_rust_rails_unrecorded_booking\.sql"#,
     r#"(a) parity's setup runs the released A28 migration file (PLAN-B11-9 r1.3)"#),
    (r#"rust/tests/fixtures/b11_9_beta/.*"#,
     WHOLE,
     r#"(b) B11-9's frozen beta installs (PLAN-B11-9 r3.4); PR2 keeps them as its upgrade input"#),
    (r#"rust/tests/(cli|rust_migrations)\.rs"#,
     r#"a JSON column Rails did not write|last_error naming Rails|'%Rails%'"#,
     r#"(b) the beta fixture's Rails-worded last_error: its generator and its probe (PLAN-B11-9 r1.1, r3.4)"#),
    (r#"rust/tests/recorded/.*"#,
     WHOLE,
     r#"(c) the recorded 2.x answers, frozen at rails-oracle-final"#),
    (r#"rust/tests/mail_parity\.rs"#,
     r#"case\["rails"\]"#,
     r#"(c2) reads the "rails" key of recorded/mails/cases.json"#),
    (r#"rust/tests/tick_basket\.rs"#,
     r#"scenario\["rails_floor"\]"#,
     r#"(c2) reads the "rails_floor" key of the recorded decisions-grid scenarios"#),
    (r#"rust/schema/(baseline|queue_baseline)\.sql"#,
     WHOLE,
     r#"(d) 2.x's schema as 2.70.21 loads it (fk_rails_* constraint names; tests/schema_baseline.rs writes the header)"#),
    (r#"rust/tests/fixtures/settings_(primary|queue)_schema\.sql"#,
     r#"fk_rails_[0-9a-f]{10}"#,
     r#"(d) 2.x's constraint names"#),
    (r#"rust/(src/rotation|tests/rotation)\.rs|rust/tests/fixtures/encryption_vectors\.json"#,
     r#"bin/rails deltabadger:encryption:reset"#,
     r#"(d) 2.x's stderr line that 3.0's encryption-reset rewrites (PLAN-B11 r9.6)"#),
    (r#"rust/tests/fixtures/setup_vectors\.json"#,
     WHOLE,
     r#"(d) 2.x's recorded setup pages and debug pages (PLAN-B11 r9.6)"#),
    (r#"rust/tests/fixtures/figures_vectors\.json"#,
     r#""ruby":"[0-9.]+""#,
     r#"(d) the interpreter version the vectors were recorded with"#),
    (r#"rust/(src|tests)/.*\.rs"#,
     r#"_aj_ruby2_keywords"#,
     r#"(d) ActiveJob's argument key inside 2.x queue rows"#),
    (r#"rust/src/venue/(http|kraken)\.rs|rust/tests/(http_proxy_env|kraken)\.rs|rust/tests/fixtures/kraken_signing\.json"#,
     r#""(Honeymaker )?Ruby""#,
     r#"(d) the User-Agent the venues have always seen (kraken_signing.json pins it)"#),
    (r#"package\.json|bun\.lock|rust/web/javascript/application\.js|script/rust/(browser_check|quiet_revisit_check)\.mjs|rust/(src|tests)/.*\.rs"#,
     r#"@hotwired/turbo-rails|@rails/actioncable[A-Za-z0-9_/.-]*|\bturbo-rails\b"#,
     r#"(e) third-party packages: turbo-rails' <turbo-cable-stream-source> and its Action Cable client"#),
    (r#".*"#,
     r#"rails-oracle-final"#,
     r#"(f) the git tag the recordings were frozen from"#),
    (r#"rust/tests/fixtures/v2/[a-z_]+\.rb|rust/tests/fixtures/umbrel/[a-z0-9.-]+\.(rb|yml)"#,
     WHOLE,
     r#"(g) 2.x programs and composes the T8 interop tests run inside 2.x's image"#),
    (r#"rust/tests/common/v2\.rs"#,
     r#"RAILS_ENV|RAILS_LOG_TO_STDOUT|bundle exec rails|`rails <args>`|`rails`"#,
     r#"(g) the 2.70.21 image command (T8)"#),
    (r#"rust/tests/image\.rs"#,
     r#"- ruby\n|`rails runner`|rust_rails_wakes"#,
     r#"(g) the 2.34.0/2.70.21 image commands (T8)"#),
    (r#"rust/tests/venue_signing\.rs"#,
     r#"ruby -ropenssl"#,
     r#"(g) the 2.70.21 image's interpreter (T8)"#),
    (r#"deltabadger/docker-compose\.yml"#,
     r#"RAILS_[A-Z_]+"#,
     r#"(g) the Umbrel app still runs the 2.70.21 image until B5's switch (#935)"#),
    (r#"rust/tests/fixtures/(seed_assets_b11|seeds_expected)\.json"#,
     r#"(?i)"(?:text:)?(?:rails|ruby-coin|ruby coin|railseur|railsusd)"|coins/(?:rails|ruby-coin)\b|RAILS_TOKEN_LOGO|"(?:text:)?RUBY""#,
     r#"(h) data: coins named Rails and Ruby Coin in the catalogue"#),
    (r#"\.github/workflows/rust\.yml"#,
     r#"Make Ruby absent[\s\S]*?no Ruby anywhere on PATH[^\n]*"#,
     r#"(h) the step that proves no Ruby is on the runner"#),
    (r#"src-tauri/src/tests\.rs"#,
     r#""RAILS_ENV""#,
     r#"(h) asserts the desktop env no longer sets it"#),
    (r#"src-tauri/tests/desktop_config\.rs"#,
     r#"for ruby in \["ruby", "bundle exec", "rails", "gem "\][\s\S]*?mentions \{ruby:\?\}"#,
     r#"(h) asserts the desktop build runs none of these"#),
    (r#"rust/tests/i18n\.rs"#,
     r#"\(\?i\)\\brails\\b|\\bRails\\b"#,
     r#"(h) i18n.rs's own scan for the word in templates and source literals"#),
    (r#"\.gitignore"#,
     r#"\.ruby-lsp"#,
     r#"(h) 2.x leftovers in existing checkouts stay ignored (PLAN-B11 r11.5)"#),
    (r#"rust/web/javascript/controllers/form/html5_validations_controller\.js"#,
     r#"modern-rails"#,
     r#"(h) a cited article's URL"#),
    (r#"rust/tests/vocabulary\.rs"#,
     WHOLE,
     r#"(h) this test names the words it scans for"#),
];

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
