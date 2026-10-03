use super::common;
use deltabadger::figures::{at::At, page_market::{Cache, Reader}};
use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use serde_json::Value;
use std::cell::RefCell;
use std::path::Path;

struct Wire { script: Value, calls: RefCell<Vec<String>> }
impl Transport for Wire {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let picked: Vec<_> = r.query.iter().filter(|(k,_)| r.path.ends_with("/bars") && (*k == "adjustment" || *k == "symbols")).collect();
        let suffix = if picked.is_empty() { String::new() } else { format!("?{}", picked.iter().map(|(k,v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")) };
        let key = format!("GET data.alpaca.markets{}{suffix}", r.path);
        self.calls.borrow_mut().push(key.clone());
        let reply = self.script.get(&key).unwrap_or_else(|| panic!("unscripted {key}"));
        Ok(HttpResponse { status: reply["status"].as_u64().unwrap_or(200) as u16, body: reply["body"].as_str().map(str::to_string).unwrap_or_else(|| reply["body"].to_string()) })
    }
}
#[tokio::test]
async fn figures_fragments_match_the_rails_page_partials() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(root.join("bin/rails")).current_dir(root)
        .args(["runner", "script/rust/pages.rb", "figures", scratch.path().to_str().unwrap()])
        .env("APP_ROOT_URL", "http://localhost:3000").env("SKIP_TEST_DATABASE", "true").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    for entry in std::fs::read_dir(scratch.path()).unwrap() {
        let dir = entry.unwrap().path();
        let name = dir.file_name().unwrap().to_str().unwrap();
        let sc: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("scenario.json")).unwrap()).unwrap();
        let expected: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("rails.json")).unwrap()).unwrap();
        let c = rusqlite::Connection::open_with_flags(dir.join("production.sqlite3"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let now = At::from_utc(chrono::DateTime::parse_from_rfc3339(sc["at"].as_str().unwrap()).unwrap().to_utc()).unwrap();
        let wire = Wire { script: sc["script"].clone(), calls: RefCell::default() };
        let mut cache = Cache::default();
        for _ in 0..4 {
            let reader = Reader::new(&cache, now.utc().timestamp());
            let _ = deltabadger::web::figure::account(&c, sc["user_id"].as_i64().unwrap(), &reader, now, "en", "token", "");
            let demands = reader.demands();
            if demands.is_empty() { break; }
            cache.fill(&wire, demands, now.utc().timestamp()).await;
        }
        let reader = Reader::new(&cache, now.utc().timestamp());
        let actual = deltabadger::web::figure::account(&c, sc["user_id"].as_i64().unwrap(), &reader, now, "en", "token", "").unwrap();
        let components = std::env::var("FIGURE_PARTS").unwrap_or_else(|_| "tile".into());
        if components.split(',').any(|p| p == "account") {
            let got = actual["account"].as_str().unwrap();
            if ["price_untraded", "index_rotation", "old_rows"].contains(&name) { assert!(got.contains("no-value")); }
            else { assert_eq!(common::html::normalize(got),common::html::normalize(expected["account"].as_str().unwrap()),"{name} account"); }
        }
        for (id, parts) in expected["bots"].as_object().unwrap() {
            for part in components.split(',').filter(|p| *p != "account") {
                let got = &actual["bots"][id][part];
                if name == "price_untraded" || name == "index_rotation" || name == "old_rows" { assert!(got.as_str().unwrap().contains("no-value"), "{name} {part}"); continue; }
                let mut got = got.as_str().unwrap().to_string();
                if ["stranded_offset", "liquidations"].contains(&name) && part == "metrics" {
                    let refusal = "<p role=\"status\">Redeploy unavailable</p>\n";
                    assert!(got.contains(refusal));
                    assert_ne!(expected["offset"][0], expected["offset"][1]);
                    got = got.replacen(refusal, "", 1);
                }
                assert_eq!(common::html::normalize(&got), common::html::normalize(parts[part].as_str().unwrap()), "{name} bot {id} {part}");
            }
        }
    }
}
