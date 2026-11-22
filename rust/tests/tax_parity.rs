//! The crypto tax report against 2.x (recorded in fixtures/tax_vectors.json): each scenario's
//! install is rebuilt from the 2.x schema and the rows 2.x read, and the report must be 2.x's own bytes or refusal,
//! or, for a scenario that says "diverges: D-TAX-…", the Rust file checked by hand and committed in
//! fixtures/tax_divergences/<name>.csv (TAX_DIVERGENCES_WRITE=1 writes them afresh, for review).
use deltabadger::figures::FiguresError;
use deltabadger::tax::{crypto_report, Outcome};
use rusqlite::{types::Value as Sql, Connection};
use serde_json::{json, Value};
use std::path::PathBuf;

fn vectors() -> Value {
    serde_json::from_str(include_str!("fixtures/tax_vectors.json")).expect("tax vectors are JSON")
}

fn install(tables: &Value) -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(include_str!("fixtures/settings_primary_schema.sql")).unwrap();
    c.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    // Rust's own NBP tables (D-TAX-PL-NBP), from the migration that creates them.
    c.execute_batch(include_str!("../migrations/20261103000001_nbp_rates.sql")).unwrap();
    // The PL carry the user declares (D-TAX-PL-DECLARED-CARRY).
    c.execute_batch(include_str!("../migrations/20261104000001_rust_tax_pl_carry.sql")).unwrap();
    // The ČNB fixings and their coverage (D-TAX-CZ-CNB).
    c.execute_batch(include_str!("../migrations/20261105000001_cnb_rates.sql")).unwrap();
    // The ESTV Kursliste values (D-TAX-CH-ESTV).
    c.execute_batch(include_str!("../migrations/20261106000001_estv_crypto_values.sql")).unwrap();
    for (table, rows) in tables.as_object().unwrap() {
        for row in rows.as_array().unwrap() {
            let row = row.as_object().unwrap();
            let columns: Vec<String> = row.keys().map(|k| format!("\"{k}\"")).collect();
            let marks = vec!["?"; row.len()].join(", ");
            let values: Vec<Sql> = row.values().map(|v| match v {
                Value::Null => Sql::Null,
                Value::Bool(b) => Sql::Integer(i64::from(*b)),
                Value::Number(n) if n.is_i64() => Sql::Integer(n.as_i64().unwrap()),
                Value::Number(n) => Sql::Real(n.as_f64().unwrap()),
                Value::String(s) => Sql::Text(s.clone()),
                other => Sql::Text(other.to_string()),
            }).collect();
            c.execute(&format!("INSERT INTO {table} ({}) VALUES ({marks})", columns.join(", ")), rusqlite::params_from_iter(values)).unwrap();
        }
    }
    c
}

/// A scenario's NBP and ČNB rates and coverage and its ESTV values, as given: coverage is never inferred from the rates.
fn nbp(c: &Connection, s: &Value) {
    for r in s["cnb"].as_array().into_iter().flatten() {
        c.execute("INSERT INTO cnb_rates (currency, date, rate) VALUES (?1, ?2, ?3)", rusqlite::params![r[0].as_str(), r[1].as_str(), r[2].as_str()]).unwrap();
    }
    for r in s["cnb_coverage"].as_array().into_iter().flatten() {
        c.execute("INSERT INTO cnb_coverage (from_date, to_date) VALUES (?1, ?2)", rusqlite::params![r[0].as_str(), r[1].as_str()]).unwrap();
    }
    // [year, symbol, value or null]: what the Kursliste said (null: asked, not listed).
    for r in s["estv"].as_array().into_iter().flatten() {
        c.execute("INSERT INTO estv_crypto_values (year, symbol, value) VALUES (?1, ?2, ?3)", rusqlite::params![r[0].as_i64(), r[1].as_str(), r[2].as_str()]).unwrap();
    }
    for r in s["nbp"].as_array().into_iter().flatten() {
        c.execute("INSERT INTO nbp_rates (currency, date, rate) VALUES (?1, ?2, ?3)", rusqlite::params![r[0].as_str(), r[1].as_str(), r[2].as_str()]).unwrap();
    }
    for r in s["nbp_coverage"].as_array().into_iter().flatten() {
        c.execute("INSERT INTO nbp_coverage (currency, from_date, to_date) VALUES (?1, ?2, ?3)", rusqlite::params![r[0].as_str(), r[1].as_str(), r[2].as_str()]).unwrap();
    }
    // [user, year, costs]: the scenario's user ("self") or another ("other").
    let me = s["user_id"].as_i64().unwrap();
    for r in s["pl_carry"].as_array().into_iter().flatten() {
        let user = if r[0] == "self" { me } else { me + 1 };
        c.execute("INSERT INTO rust_tax_pl_carry (user_id, year, costs, updated_at) VALUES (?1, ?2, ?3, '2026-09-10 12:00:00')",
                  rusqlite::params![user, r[1].as_i64(), r[2].as_str()]).unwrap();
    }
}

fn v2(s: &Value) -> Outcome {
    let v2 = &s["expected"];
    match v2.get("csv") {
        Some(csv) => Outcome::Csv(csv.as_str().unwrap().to_string()),
        None => Outcome::Refused {
            reason: v2["refused"]["reason"].as_str().unwrap().to_string(),
            symbols: v2["refused"]["symbols"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect(),
        },
    }
}

fn divergence(name: &str) -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tax_divergences").join(format!("{name}.csv")) }

#[test]
fn the_crypto_report_is_v2_own_or_the_named_divergence() {
    let v = vectors();
    let scenarios = v["scenarios"].as_array().unwrap();
    assert!(scenarios.len() >= 147);
    let write = std::env::var("TAX_DIVERGENCES_WRITE").is_ok();
    let mut failures = vec![];
    // The CoinGecko scenarios ask the feed; `coingecko_reports_*` replays them.
    for s in scenarios.iter().filter(|s| s["scope"] != "broker" && s["coingecko"].is_null()) {
        let name = s["name"].as_str().unwrap();
        let country = s["country"].as_str().unwrap();
        let c = install(&s["tables"]);
        nbp(&c, s);
        let year = s["year"].as_i64().unwrap();
        let today = chrono::NaiveDate::parse_from_str(&s["now"].as_str().unwrap()[..10], "%Y-%m-%d").unwrap();
        let got = crypto_report(&c, s["user_id"].as_i64().unwrap(), country, year as i32,
                                s["stablecoin_as_fiat"].as_bool().unwrap(), today);
        // Recorded for its later slice: a jurisdiction this build does not serve states no figure.
        if !deltabadger::tax::jurisdictions::ported(country, year) {
            if !matches!(got, Err(FiguresError::NotComputed(_))) { failures.push(format!("{name}: {country} is not ported, got {got:?}")); }
            continue;
        }
        let got = match got { Ok(g) => g, Err(e) => { failures.push(format!("{name}: {e:?}")); continue } };
        let expect = s["expect"].as_str().unwrap();
        // "refuses: D-TAX-… <reason> <symbols…>": Rust refuses where 2.x writes a file.
        if let Some(rest) = expect.strip_prefix("refuses: ") {
            let mut words = rest.split(' ');
            let (id, reason) = (words.next().unwrap(), words.next().unwrap().to_string());
            assert!(id.starts_with("D-TAX-"), "{name}: {id}");
            let want = Outcome::Refused { reason, symbols: words.map(String::from).collect() };
            if got != want { failures.push(format!("{name}: {id}: {got:?}")); }
            continue;
        }
        match expect.strip_prefix("diverges: ") {
            None => if got != v2(s) { failures.push(format!("{name}: not 2.x's file\n--- 2.x\n{:?}\n--- rust\n{got:?}", v2(s))) },
            Some(id) => {
                assert!(id.starts_with("D-TAX-"), "{name}: {id}");
                let Outcome::Csv(csv) = &got else { failures.push(format!("{name}: refused, {got:?}")); continue };
                if *csv == match v2(s) { Outcome::Csv(r) => r, _ => String::new() } { failures.push(format!("{name}: {id} names a divergence, but Rust wrote 2.x's file")); }
                let path = divergence(name);
                if write { std::fs::create_dir_all(path.parent().unwrap()).unwrap(); std::fs::write(&path, csv).unwrap(); }
                match std::fs::read_to_string(&path) {
                    Ok(want) if want == *csv => {}
                    Ok(want) => failures.push(format!("{name}: not the reviewed {id} file\n--- reviewed\n{want}\n--- rust\n{csv}")),
                    Err(e) => failures.push(format!("{name}: {}: {e}", path.display())),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} of {} scenarios:\n{}", failures.len(), scenarios.len(), failures.join("\n\n"));
}

/// D-TAX-PL-DECLARED-CARRY: a declaration for a later year is not read, so the 2024 file is the one without it.
#[test]
fn a_later_years_declared_carry_is_not_read() {
    let file = |name: &str| std::fs::read_to_string(divergence(name)).unwrap();
    assert_eq!(file("pl_declared_future_ignored_2024"), file("pl_carry_two_years_2024"));
}

/// Every reviewed divergence file belongs to a scenario that says so.
#[test]
fn no_divergence_file_is_left_over() {
    let v = vectors();
    let named: std::collections::HashSet<String> = v["scenarios"].as_array().unwrap().iter()
        .filter(|s| s["expect"].as_str().unwrap().starts_with("diverges: ") && deltabadger::tax::jurisdictions::ported(s["country"].as_str().unwrap(), s["year"].as_i64().unwrap())).map(|s| format!("{}.csv", s["name"].as_str().unwrap())).collect();
    for f in std::fs::read_dir(divergence("x").parent().unwrap()).unwrap() {
        let f = f.unwrap().file_name().into_string().unwrap();
        assert!(named.contains(&f), "{f} has no diverging scenario");
    }
}

#[test]
fn jurisdictions_not_yet_ported_state_no_figure() {
    let c = install(&serde_json::json!({}));
    let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
    // Every jurisdiction is served (A12-6); an unknown one states no figure.
    assert!(matches!(crypto_report(&c, 1, "XX", 2026, false, today), Err(FiguresError::NotComputed(m)) if m.contains("jurisdiction XX")));
    // Poland's regime starts in 2019.
    assert!(matches!(crypto_report(&c, 1, "PL", 2018, false, today), Err(FiguresError::NotComputed(m)) if m.contains("jurisdiction PL")));
    // D-TAX-GR-NO-TAX: Greece is served up to 2026 only, until enacted law is reviewed.
    assert!(matches!(crypto_report(&c, 1, "GR", 2027, false, today), Err(FiguresError::NotComputed(m)) if m.contains("jurisdiction GR")));
    // D-TAX-IT-YEARS: Italy 2023–2026; D-TAX-IE-YEARS: Ireland 2013–2026; D-TAX-SE-YEARS, D-TAX-GB-YEARS: 2020–2026.
    for (country, year) in [("IT", 2022), ("IT", 2027), ("IE", 2012), ("IE", 2027), ("SE", 2019), ("SE", 2027), ("GB", 2019), ("GB", 2027),
                          // D-TAX-FR-YEARS 2019–2026, D-TAX-NL-YEARS 2021–2026, D-TAX-CH-YEARS 2020–2026.
                          ("FR", 2018), ("FR", 2027), ("NL", 2020), ("NL", 2027), ("CH", 2019), ("CH", 2027)] {
        assert!(matches!(crypto_report(&c, 1, country, year, false, today), Err(FiguresError::NotComputed(m)) if m.contains(&format!("jurisdiction {country}"))), "{country} {year}");
    }
}

/// The text before the broker report's legal notes (D-TAX-BROKER-DISCLOSURES: every broker file differs there).
fn before_disclosures(csv: &str) -> &str {
    let header = deltabadger::web::i18n::text("de", "tax_report.broker.sections.disclosures", &[]);
    csv.find(&format!("\n{header}\n")).map_or(csv, |at| &csv[..at])
}

/// The legal notes every broker file ends with, exactly (2.x's CSV quotes a cell holding a comma or a quote).
fn notes() -> String {
    let t = |key: &str| deltabadger::web::i18n::text("de", key, &[]);
    let cell = |v: String| if v.contains([',', '"']) { format!("\"{}\"", v.replace('"', "\"\"")) } else { v };
    let mut out = format!("\n{}\n", t("tax_report.broker.sections.disclosures"));
    for key in deltabadger::tax::broker::DISCLOSURES { out.push_str(&cell(t(key))); out.push('\n'); }
    out
}

/// PLAN-A12 A12-7: the German broker report against 2.x (`scope: broker` scenarios). "parity": 2.x's bytes up to the legal
/// notes, then this build's notes (D-TAX-BROKER-DISCLOSURES); "diverges: D-TAX-BROKER-…": the reviewed file; "failed": the
/// run fails as 2.x's ArgumentError does. "fetch": 2.x's side of a fetch, whose Rust side is tax_job.rs's.
#[test]
fn the_broker_report_is_v2_own_or_the_named_divergence() {
    let v = vectors();
    let scenarios: Vec<&Value> = v["scenarios"].as_array().unwrap().iter().filter(|s| s["scope"] == "broker").collect();
    assert!(scenarios.len() >= 30, "{}", scenarios.len());
    let write = std::env::var("TAX_DIVERGENCES_WRITE").is_ok();
    let mut failures = vec![];
    for s in scenarios {
        let name = s["name"].as_str().unwrap();
        let expect = s["expect"].as_str().unwrap();
        if expect == "fetch" { continue; }
        let c = install(&s["tables"]);
        let (user, year) = (s["user_id"].as_i64().unwrap(), s["year"].as_i64().unwrap());
        let checked = deltabadger::tax::broker::check(&c, user, s["country"].as_str().unwrap(), year);
        if expect == "failed" {
            if s["expected"].get("failed").is_none() { failures.push(format!("{name}: 2.x did not fail")); }
            if checked.is_ok() { failures.push(format!("{name}: the run must fail")); }
            continue;
        }
        if let Err(e) = checked { failures.push(format!("{name}: {e}")); continue; }
        let csv = match deltabadger::tax::broker::report(&c, user, year as i32, &Default::default()) { Ok(csv) => csv, Err(e) => { failures.push(format!("{name}: {e:?}")); continue } };
        let v2 = s["expected"]["csv"].as_str().unwrap_or("");
        // D-TAX-BROKER-DISCLOSURES: this build's legal notes, every one, and none of the three 2.x states that no longer hold.
        if csv[before_disclosures(&csv).len()..] != notes() { failures.push(format!("{name}: not this build's legal notes")); }
        match expect.strip_prefix("diverges: ") {
            None => if before_disclosures(&csv) != before_disclosures(v2) {
                failures.push(format!("{name}: not 2.x's file\n--- 2.x\n{v2}\n--- rust\n{csv}"));
            },
            Some(id) => {
                assert!(id.starts_with("D-TAX-BROKER-"), "{name}: {id}");
                if before_disclosures(&csv) == before_disclosures(v2) { failures.push(format!("{name}: {id} names a divergence, but Rust wrote 2.x's figures")); }
                let path = divergence(name);
                if write { std::fs::create_dir_all(path.parent().unwrap()).unwrap(); std::fs::write(&path, &csv).unwrap(); }
                match std::fs::read_to_string(&path) {
                    Ok(want) if want == csv => {}
                    Ok(want) => failures.push(format!("{name}: not the reviewed {id} file\n--- reviewed\n{want}\n--- rust\n{csv}")),
                    Err(e) => failures.push(format!("{name}: {}: {e}", path.display())),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}:\n{}", failures.len(), failures.join("\n\n"));
}

/// One CoinGecko scenario replayed as the tax job runs it (`tracker::jobs::fetching` over a CoinGecko feed of the answers
/// 2.x's Clients::Coingecko was given, demo plan seeded as 2.x's was): the report, the prices stored
/// (asset, currency, date, price) and the requests, and the feed after the run.
async fn coingecko_replay(s: &Value) -> (Outcome, Vec<(String, String, String, f64)>, Value, bool) {
    use deltabadger::jobs::{coingecko::{CoinGecko, Plan, Throttle}, market::Feed, Db};
    let mut tables = s["tables"].clone();
    tables["historical_prices"] = serde_json::json!([]); // the scenarios store none before the run: what is there is 2.x's writes
    let c = install(&tables);
    nbp(&c, s);
    let today = chrono::NaiveDate::parse_from_str(&s["now"].as_str().unwrap()[..10], "%Y-%m-%d").unwrap();
    let now: chrono::DateTime<chrono::Utc> = s["now"].as_str().unwrap().parse().unwrap();
    let prepared = deltabadger::tax::prepare(&c, s["user_id"].as_i64().unwrap(), s["country"].as_str().unwrap(), s["year"].as_i64().unwrap() as i32,
                                             s["stablecoin_as_fiat"].as_bool().unwrap()).unwrap();
    let deltabadger::tax::Prepared::Report(report) = prepared else { panic!("{}: nothing to fetch for", s["name"]) };
    let db = Db::new(c, deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "tax-parity").unwrap()));
    let t = deltabadger::venue::http::ScriptedTransport::from_script(&s["coingecko"]["script"]);
    let feed = Feed::CoinGecko(CoinGecko::new(s["coingecko"]["key"].as_str().unwrap(), t.clone())
        .with_throttle(std::sync::Arc::new(Throttle::new(usize::MAX))).with_plan(Plan::Demo));
    let got = deltabadger::tracker::jobs::fetching(&db, Some(&feed), now, &mut deltabadger::tracker::jobs::Allowance::run(),
                                                   move |fetcher| report.compute(fetcher, today, &mut |_| {})).await.unwrap().unwrap();
    let prices = db.run(|c, _| {
        let mut q = c.prepare("SELECT asset, currency, date, CAST(price AS REAL) FROM historical_prices ORDER BY id").map_err(|e| e.to_string())?;
        let rows = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map_err(|e| e.to_string())?.collect::<Result<Vec<_>, _>>();
        rows.map_err(|e| e.to_string())
    }).await.unwrap();
    (got, prices, Value::Array(deltabadger::tracker::parity::logged(&t)), feed.rate_limited())
}

fn v2_prices(s: &Value) -> Vec<(String, String, String, f64)> {
    s["tables"]["historical_prices"].as_array().unwrap().iter().map(|r| {
        let price = r["price"].as_f64().or_else(|| r["price"].as_str().and_then(|p| p.parse().ok())).unwrap();
        (r["asset"].as_str().unwrap().into(), r["currency"].as_str().unwrap().into(), r["date"].as_str().unwrap().into(), price)
    }).collect()
}

/// PLAN-A17 A17-3: a CoinGecko install's report against 2.x's own (the recording's CoinGecko mode, the real
/// Tax::PriceService beneath scripted api.coingecko.com): the file, the prices stored and the requests are 2.x's, except
/// D-CG-429-STOPS-PRICES, whose differences are asserted.
#[tokio::test(flavor = "current_thread")]
async fn coingecko_reports_are_v2_own_or_the_429_stop() {
    let v = vectors();
    let cg: Vec<&Value> = v["scenarios"].as_array().unwrap().iter().filter(|s| !s["coingecko"].is_null()).collect();
    assert_eq!(cg.len(), 4, "the CoinGecko scenarios");
    for s in cg {
        let name = s["name"].as_str().unwrap();
        let (got, prices, requests, limited) = coingecko_replay(s).await;
        let v2_requests: Value = s["coingecko"]["requests"].clone();
        if s["expect"] == "parity" {
            assert_eq!(got, v2(s), "{name}: the report");
            assert_eq!(prices, v2_prices(s), "{name}: the prices stored");
            assert_eq!(requests, v2_requests, "{name}: the requests");
            assert!(!limited, "{name}");
            continue;
        }
        assert_eq!((name, s["expect"].as_str().unwrap()), ("cg_429_then_later_coin", "diverges: D-CG-429-STOPS-PRICES"));
        // 2.x: BTC's 429, then ETH asked and stored, then BTC's single window asked again.
        let path = |r: &Value| r[0].as_str().unwrap().to_string();
        assert_eq!(v2_requests.as_array().unwrap().iter().map(path).collect::<Vec<_>>(),
                   ["GET /api/v3/coins/bitcoin/market_chart/range", "GET /api/v3/coins/ethereum/market_chart/range", "GET /api/v3/coins/bitcoin/market_chart/range"]);
        assert_eq!(v2_prices(s).iter().map(|p| (p.0.as_str(), p.2.as_str())).collect::<Vec<_>>(), [("ETH", "2026-04-10")]);
        // Rust: the first request's 429 stops the run asking; nothing stored; the report still completes, ETH warned too.
        assert_eq!(requests, Value::Array(v2_requests.as_array().unwrap()[..1].to_vec()));
        assert!(prices.is_empty() && limited);
        let (Outcome::Csv(rust), Outcome::Csv(v2_csv)) = (&got, v2(s)) else { panic!("{name}: {got:?}") };
        assert!(v2_csv.contains("WARNING: Missing historical prices for:\nBTC/USD 2026-02-10\nUpgrade"), "{v2_csv}");
        assert!(rust.contains("WARNING: Missing historical prices for:\nBTC/USD 2026-02-10\nETH/USD 2026-04-10\nUpgrade"), "{rust}");
        assert!(rust.contains("2 values missing") && v2_csv.contains("1 values missing"), "{rust}");
        assert!(v2_csv.contains(",ETH,1.0,2600.0,2599.0,1.0,USD,") && rust.contains(",ETH,1.0,2600.0,0.0,2600.0,USD,"), "{rust}");
    }
}

/// D-TAX-CFEE-PREVIOUS-DAY: 2.x folded a CFEE into the fills of its own day. Moved one day later, the same fee is
/// read as charged on those same fills, and the report is the file 2.x (or the reviewed divergence) wrote for them.
#[test]
fn a_cfee_dated_the_day_after_folds_as_v2_folded_it_on_the_day() {
    let v = vectors();
    // The scenarios whose fee folded on its day (or, GB, whose file prints no fee date); the others print the fee's date.
    let names = ["cfee_into_buys", "cfee_into_a_sale", "it_pre2023_cfee", "gb_cfee_lookahead_end"];
    let mut failures = vec![];
    let mut seen = 0;
    for s in v["scenarios"].as_array().unwrap() {
        let name = s["name"].as_str().unwrap();
        let has_cfee = s["tables"]["account_transactions"].as_array().is_some_and(|r| r.iter().any(|t| t["raw_data"].as_str().is_some_and(|d| d.contains("CFEE"))));
        if !has_cfee || !names.contains(&name) { continue; }
        seen += 1;
        let mut tables = s["tables"].clone();
        for t in tables["account_transactions"].as_array_mut().unwrap() {
            if !t["raw_data"].as_str().is_some_and(|d| d.contains("CFEE")) { continue; }
            let at = chrono::NaiveDateTime::parse_from_str(t["transacted_at"].as_str().unwrap(), "%Y-%m-%d %H:%M:%S").unwrap() + chrono::Duration::days(1);
            t["transacted_at"] = Value::String(at.format("%Y-%m-%d %H:%M:%S").to_string());
        }
        let c = install(&tables);
        nbp(&c, s);
        let today = chrono::NaiveDate::parse_from_str(&s["now"].as_str().unwrap()[..10], "%Y-%m-%d").unwrap();
        let got = crypto_report(&c, s["user_id"].as_i64().unwrap(), s["country"].as_str().unwrap(), s["year"].as_i64().unwrap() as i32,
                                s["stablecoin_as_fiat"].as_bool().unwrap(), today).unwrap();
        // What was written for the fee on its own day: 2.x's file, or the file reviewed before D-TAX-CFEE-PREVIOUS-DAY.
        let want = match name { "cfee_into_buys" | "cfee_into_a_sale" => v2(s), _ => Outcome::Csv(std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tax_cfee_day_of").join(format!("{name}.csv"))).unwrap()) };
        if got != want { failures.push(format!("{name}\n--- want\n{want:?}\n--- got\n{got:?}")); }
    }
    assert_eq!(seen, names.len());
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

fn scenario(name: &str) -> Value { vectors()["scenarios"].as_array().unwrap().iter().find(|s| s["name"] == name).unwrap().clone() }
fn report(s: &Value, tables: &Value, country: &str) -> String {
    let c = install(tables);
    nbp(&c, s);
    let today = chrono::NaiveDate::parse_from_str(&s["now"].as_str().unwrap()[..10], "%Y-%m-%d").unwrap();
    match crypto_report(&c, s["user_id"].as_i64().unwrap(), country, s["year"].as_i64().unwrap() as i32, s["stablecoin_as_fiat"].as_bool().unwrap(), today).unwrap() {
        Outcome::Csv(csv) => csv,
        other => panic!("{other:?}"),
    }
}
fn set(tables: &mut Value, id: i64, field: &str, value: Value) {
    let t = tables["account_transactions"].as_array_mut().unwrap().iter_mut().find(|t| t["id"] == id).unwrap();
    t[field] = value;
}

/// D-TAX-CFEE-PREVIOUS-DAY: the fold runs on the whole history before the year is cut. A New York taxpayer's sale at
/// 02:00 UTC on 1 January is in their 2024 year, and the fee Alpaca dates 2 January (past the year's rows) still goes
/// to it: 0.15 ETH leave for the sale's 50 dollars.
#[test]
fn a_cfee_dated_past_the_year_end_still_folds_into_the_years_last_fills() {
    let s = scenario("us_cfee_new_york_year_end");
    let mut tables = s["tables"].clone();
    set(&mut tables, 1, "entry_type", json!(1));
    set(&mut tables, 2, "transacted_at", json!("2025-01-03 12:00:00"));
    set(&mut tables, 4, "transacted_at", json!("2025-01-02 00:00:00"));
    let csv = report(&s, &tables, "US");
    assert!(csv.contains(",ETH,0.15,50.0,270.0,"), "{csv}");
    assert!(!csv.contains("Fees paid in an asset"), "{csv}");
}

/// Expense mode (GB): each fill's share of the fee is the fee valued as its own row is: a stated price wins. At twice
/// the fills' price, the 30-day match's share doubles from 98 to 196 pounds.
#[test]
fn a_cfee_expense_is_the_fee_valued_as_its_own_row() {
    let s = scenario("gb_cfee_lookahead_end");
    let mut tables = s["tables"].clone();
    set(&mut tables, 5, "transacted_at", json!("2025-05-06 00:00:00"));
    assert!(report(&s, &tables, "GB").contains(",BTC,0.49,15000.0,9898.0,"));
    set(&mut tables, 5, "manual_values", json!("{\"price\":\"51764.72\"}"));
    let csv = report(&s, &tables, "GB");
    assert!(csv.contains(",BTC,0.49,15000.0,9996.0,"), "{csv}");
}

/// Poland and the wealth reports never folded a CFEE, and still do not: the Dutch file is the one for a plain fee row.
#[test]
fn poland_and_the_wealth_reports_keep_a_cfee_a_row_of_its_own() {
    let s = scenario("cfee_into_buys");
    let mut tables = s["tables"].clone();
    set(&mut tables, 3, "transacted_at", json!("2026-01-06 00:00:00"));
    let mut plain = tables.clone();
    set(&mut plain, 3, "raw_data", json!("{}"));
    assert_eq!(report(&s, &tables, "NL"), report(&s, &plain, "NL"));
    assert!(report(&s, &tables, "PL").contains("0.0025"));
}

/// D-TAX-CFEE-MATCHED-FILL: a CFEE whose raw `created_at` says when it was charged, and the fills' raw prices.
fn timed_cfee(tables: &mut Value, fee: i64, created: &str, price: &str, fills: &[(i64, &str)]) {
    set(tables, fee, "raw_data", json!(json!({"activity_type": "CFEE", "created_at": created, "symbol": "BTCUSD", "price": price}).to_string()));
    for (id, price) in fills { set(tables, *id, "raw_data", json!(json!({"activity_type": "FILL", "symbol": "BTC/USD", "price": price}).to_string())); }
}

/// D-TAX-CFEE-MATCHED-FILL: a timed fee booked at 00:00 of its fill's day folds into that fill: the file 2.x wrote with
/// the fee on its day (a sale), and the one reviewed for it (Italy's pre-2023 fold into the buy).
#[test]
fn a_timed_cfee_folds_into_its_matched_fill() {
    let s = scenario("cfee_into_a_sale");
    let mut tables = s["tables"].clone();
    timed_cfee(&mut tables, 3, "2026-01-06T10:00:03Z", "600", &[(1, "100"), (2, "600")]);
    let Outcome::Csv(want) = v2(&s) else { panic!() };
    assert_eq!(report(&s, &tables, "US"), want);
    let s = scenario("it_pre2023_cfee");
    let mut tables = s["tables"].clone();
    timed_cfee(&mut tables, 2, "2022-06-01T15:00:04Z", "10500", &[(1, "10500")]);
    let want = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tax_cfee_day_of/it_pre2023_cfee.csv")).unwrap();
    assert_eq!(report(&s, &tables, "IT"), want);
}

/// D-TAX-CFEE-MATCHED-FILL (GB): a timed fee is the expense of its one fill: the purchase the 30-day rule matches carries
/// the whole fee, 9,996 pounds of cost (pro rata over the day's two purchases: 9,898). Booked four days late, the fee
/// still revises the disposal while the provisional note is shown: through 10 May (5 April + 36 days), not on 11 May.
#[test]
fn a_timed_cfee_is_its_fills_expense_and_the_note_waits_for_its_booking() {
    let s = scenario("gb_cfee_lookahead_end");
    let mut tables = s["tables"].clone();
    timed_cfee(&mut tables, 5, "2025-05-05T12:00:03Z", "25882.36", &[(6, "25882.36"), (7, "25882.36")]);
    assert!(report(&s, &tables, "GB").contains(",BTC,0.49,15000.0,9996.0,"), "{}", report(&s, &tables, "GB"));
    set(&mut tables, 5, "transacted_at", json!("2025-05-09 00:00:00"));
    let c = install(&tables);
    for (today, shown) in [("2025-05-10", true), ("2025-05-11", false)] {
        let today = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d").unwrap();
        let Outcome::Csv(csv) = crypto_report(&c, s["user_id"].as_i64().unwrap(), "GB", 2024, false, today).unwrap() else { panic!() };
        assert_eq!(csv.contains("BTC 2025-04-05"), shown, "{today}: {csv}");
        assert!(csv.contains(",BTC,0.49,15000.0,9996.0,"), "{today}: {csv}");
    }
}

/// Codex review 1: Italy values a coin bought for USDC (an e-money token exchange) at its own price; a timed fee folded
/// into that buy leaves fewer units, not less cost: the 0.99 BTC sold still cost the purchase's 10,000 euros.
#[test]
fn a_fee_folded_into_a_usdc_buy_keeps_the_buys_cost_in_italy() {
    let s = scenario("it_pre2023_cfee");
    let mut tables = s["tables"].clone();
    set(&mut tables, 1, "quote_currency", json!("USDC"));
    set(&mut tables, 2, "raw_data", json!(json!({"activity_type": "CFEE", "created_at": "2022-06-01T15:00:04Z", "symbol": "BTCUSDC", "price": "10500"}).to_string()));
    set(&mut tables, 1, "raw_data", json!(json!({"activity_type": "FILL", "symbol": "BTC/USDC", "price": "10500"}).to_string()));
    let csv = report(&s, &tables, "IT");
    assert!(csv.contains(",BTC,0.99,14850.0,10000.0,"), "{csv}");
}
