//! Every pure rule of rust/src/figures against what Ruby recorded (script/rust/record_figures_vectors.rb).
use deltabadger::figures::at::At;
use deltabadger::figures::dec::Dec;
use deltabadger::figures::json::{self, J};
use deltabadger::figures::num::{float_to_s, oj_float, Num, NumError};
use serde_json::Value;

fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/figures_vectors.json")).expect("figures_vectors.json parses") }
fn float(v: &Value) -> f64 { f64::from_bits(u64::from_str_radix(v.as_str().unwrap(), 16).unwrap()) }
fn num(v: &Value) -> Num {
    if let Some(i) = v.get("i") { return Num::Int(i.as_i64().unwrap()); }
    if let Some(d) = v.get("d") { return Num::Dec(Dec::parse(d.as_str().unwrap()).unwrap()); }
    Num::Float(float(&v["f"]))
}
/// A Num as the recorder tags one, so a kind that differs is a difference.
fn tagged(n: &Num) -> Value {
    match n {
        Num::Int(i) => serde_json::json!({ "i": i }),
        Num::Dec(d) => serde_json::json!({ "d": d.to_s_f() }),
        Num::Float(f) => serde_json::json!({ "f": format!("{:016x}", f.to_bits()) }),
    }
}

#[test]
fn a_float_prints_as_ruby_prints_it_and_as_rails_json_does() {
    let v = vectors();
    let (to_s, oj) = (v["float_to_s"].as_array().unwrap(), v["oj_float"].as_array().unwrap());
    assert!(to_s.len() > 1_200 && oj.len() > 1_200);
    let wrong: Vec<String> = to_s.iter().filter(|c| float_to_s(float(&c[0])) != c[1].as_str().unwrap()).map(|c| format!("to_s {c} gave {}", float_to_s(float(&c[0]))))
        .chain(oj.iter().filter(|c| oj_float(float(&c[0])) != c[1].as_str().unwrap()).map(|c| format!("json {c} gave {}", oj_float(float(&c[0])))))
        .collect();
    assert!(wrong.is_empty(), "{} differ:\n{}", wrong.len(), wrong[..wrong.len().min(10)].join("\n"));
    // The fallback is exercised, and so is the loss: sixteen digits are not always the double Rails computed.
    assert!(oj.iter().any(|c| c[1].as_str().unwrap().len() > 17 && float_to_s(float(&c[0])) == c[1].as_str().unwrap()));
    assert!(oj.iter().any(|c| c[1].as_str().unwrap().parse::<f64>().unwrap() != float(&c[0])), "no vector shows the sixteen-digit loss");
}

#[test]
fn every_recorded_operation_keeps_rubys_value_and_kind() {
    let v = vectors();
    let cases = v["num"].as_array().unwrap();
    assert_eq!(cases.len(), 11_200);
    let (mut wrong, mut negative_zeros) = (vec![], 0);
    for c in cases {
        let (a, b) = (num(&c[1]), num(&c[2]));
        let got = match c[0].as_str().unwrap() {
            "+" => a.add(&b), "-" => a.sub(&b), "*" => a.mul(&b), "/" => a.div(&b),
            "<=>" => a.compare(&b).map(|o| Num::Int(o as i64)),
            "min" => Num::min2(a.clone(), b.clone()), "max" => Num::max2(a.clone(), b.clone()),
            other => panic!("unknown op {other}"),
        };
        let got = match &got { Ok(n) => tagged(n), Err(_) => serde_json::json!({ "raise": "not a number" }) };
        // Ruby's BigDecimal has a negative zero (0 x -1 is -0.0) and Rails prints it: the sign is part of the answer.
        if c[3]["d"] == "-0.0" { negative_zeros += 1; }
        if got != c[3] { wrong.push(format!("{c} gave {got}")); }
    }
    assert!(wrong.is_empty(), "{} of {} differ:\n{}", wrong.len(), cases.len(), wrong[..wrong.len().min(15)].join("\n"));
    assert!(negative_zeros > 100, "only {negative_zeros} answers are the negative zero");
}

#[test]
fn predicates_conversions_and_rounding_match_ruby() {
    let v = vectors();
    for c in v["predicates"].as_array().unwrap() {
        let n = num(&c[0]);
        assert_eq!((n.is_zero(), n.is_positive(), n.is_negative()), (c[1].as_bool().unwrap(), c[2].as_bool().unwrap(), c[3].as_bool().unwrap()), "{c}");
        assert_eq!(tagged(&Num::Dec(n.to_d().unwrap())), c[4], "to_d {c}");
        assert_eq!(n.to_f().to_bits(), float(&c[5]).to_bits(), "to_f {c}");
    }
    let rounds = v["round"].as_array().unwrap();
    assert!(rounds.len() >= 80);
    for c in rounds { assert_eq!(tagged(&num(&c[0]).round(c[1].as_i64().unwrap()).unwrap()), c[2], "{c}"); }
}

fn j(v: &Value) -> J {
    match v {
        Value::Null => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::Number(n) => n.as_i64().map(J::Int).unwrap_or_else(|| J::Float(n.as_f64().unwrap())),
        Value::String(s) => J::Str(s.clone()),
        Value::Array(a) => J::Arr(a.iter().map(j).collect()),
        Value::Object(o) => J::Obj(o.iter().map(|(k, v)| (k.clone(), j(v))).collect()),
    }
}

#[test]
fn json_text_and_times_are_written_as_rails_writes_them() {
    let v = vectors();
    for c in v["json"].as_array().unwrap() { assert_eq!(j(&c[0]).write(), c[1].as_str().unwrap()); }
    let times = v["times"].as_array().unwrap();
    assert_eq!(times.len(), 64);
    // A zone at no offset that is not UTC, and UTC itself.
    assert!(times.iter().any(|c| c[1] == "London" && c[3].as_str().unwrap().ends_with("+00:00\"")) && times.iter().any(|c| c[1] == "London" && c[3].as_str().unwrap().ends_with("+01:00\"")));
    assert!(times.iter().any(|c| c[1] == "UTC" && c[3].as_str().unwrap().ends_with("Z\"")));
    for c in times {
        let at = At(c[0].as_i64().unwrap()).utc();
        let zone = deltabadger::web::timezone::zone(c[1].as_str().unwrap()).unwrap();
        let text = json::time(&at.with_timezone(&zone), c[2].as_u64().unwrap() as usize);
        assert_eq!(J::Str(text).write(), c[3].as_str().unwrap(), "{c}");
    }
    // A text is read as a number only where it is a clean decimal, and then as String#to_d reads it. What Ruby
    // reads off the front of anything else ("12abc" is 12), and its Infinity and NaN, are no figure here.
    let strings = v["to_d"].as_array().unwrap();
    assert_eq!(strings.len(), 49);
    let (mut read, mut refused) = (0, vec![]);
    for c in strings {
        match Dec::strict(c[0].as_str().unwrap()) {
            Ok(d) => { assert_eq!(d.to_s_f(), c[1].as_str().unwrap(), "{:?}.to_d", c[0]); read += 1; }
            Err(error) => { assert_eq!(error, NumError::NotANumber, "{:?}", c[0]); refused.push(c[0].as_str().unwrap()); }
        }
    }
    assert_eq!((read, refused.len()), (15, 34), "refused: {refused:?}");
    assert!(["NaN", "Infinity", "-Infinity", "12abc", "abc", "", " 12 ", "1_000.5", ".5", "5."].iter().all(|text| refused.contains(text)));
    for c in v["time_minus"].as_array().unwrap() {
        assert_eq!(At(c[0].as_i64().unwrap()).minus(At(c[1].as_i64().unwrap())).to_bits(), float(&c[2]).to_bits(), "{c}");
    }
}

// ---- Bot::RebalanceAccounting, Bot::TaxLots, Bot::Composition::HoldingKeys: the app's own modules, replayed ----

use deltabadger::figures::books::{Books, Fill, Ledger};
use deltabadger::figures::keys::{self, Identity};
use deltabadger::figures::lots::{self, Lot, Lots};

fn dec(v: &Value) -> Dec { Dec::parse(v.as_str().unwrap()).unwrap() }
fn opt_dec(v: &Value) -> Option<Dec> { v.as_str().map(|s| Dec::parse(s).unwrap()) }

#[test]
fn holdings_that_share_a_symbol_get_the_keys_rails_gives_them() {
    let v = vectors();
    let cases = v["holding_keys"].as_array().unwrap();
    assert_eq!(cases.len(), 200);
    let pairs = |list: &Value| -> Vec<(Identity, String)> {
        list.as_array().unwrap().iter().map(|p| (p[0].as_i64().map_or_else(|| Identity::Text(p[0].as_str().unwrap().into()), Identity::Asset), p[1].as_str().unwrap().to_string())).collect()
    };
    let mut renamed = 0;
    for c in cases {
        let (given, want) = (pairs(&c[0]), pairs(&c[1]));
        assert_eq!(keys::call(&given), want, "{c}");
        if given != want { renamed += 1; }
    }
    assert!(renamed > 50, "only {renamed} cases had a clash to resolve");
    assert_eq!((keys::candidate(7, Some("POR"), Some("Portal")), keys::candidate(7, Some(" "), Some("Portal")), keys::candidate(7, None, Some(""))), ("POR".into(), "Portal".into(), "#7".into()));
}

fn lot_list(v: &Value) -> Lots { v.as_array().unwrap().iter().map(|l| Lot { amount: dec(&l[0]), cost: opt_dec(&l[1]) }).collect() }

#[test]
fn tax_lots_cost_lose_shrink_and_split_as_rails_computes_them() {
    let v = vectors();
    let cases = v["tax_lots"].as_array().unwrap();
    assert_eq!(cases.len(), 300);
    let (mut unknown, mut losses) = (0, 0);
    for c in cases {
        let list = lot_list(&c["lots"]);
        let (amount, proceeds) = (dec(&c["amount"]), dec(&c["proceeds"]));
        assert_eq!(tagged(&lots::basis(&list).unwrap()), c["basis"], "basis {c}");
        assert_eq!(tagged(&lots::units(&list).unwrap()), c["units"], "units {c}");
        assert_eq!(lots::unknown_cost(&list), c["unknown"].as_bool().unwrap(), "unknown {c}");
        assert_eq!(lots::cost_of(&list, &amount).unwrap().to_s_f(), c["cost_of"].as_str().unwrap(), "cost_of {c}");
        let loss = lots::loss_in(&list, &amount, &proceeds).unwrap();
        assert_eq!(loss, c["loss_in"].as_bool(), "loss_in {c}");
        match loss { None => unknown += 1, Some(true) => losses += 1, Some(false) => {} }
        let mut consumed = list.clone();
        lots::consume(&mut consumed, &amount).unwrap();
        assert_eq!(consumed, lot_list(&c["consumed"]), "consume {c}");
        let mut restated = list.clone();
        lots::split(&mut restated, &dec(&c["factor"])).unwrap();
        assert_eq!(restated, lot_list(&c["split"]), "split {c}");
    }
    assert!(unknown > 10 && losses > 10, "the three verdicts are all met: {unknown} unknown, {losses} losses");
}

#[test]
fn every_kind_of_fill_moves_the_books_as_rails_moves_them() {
    let v = vectors();
    let histories = v["books"].as_array().unwrap();
    assert_eq!(histories.len(), 250);
    let mut branches = std::collections::BTreeMap::new();
    for steps in histories {
        let (mut ledger, mut books) = (Ledger::default(), Books::default());
        for (i, step) in steps.as_array().unwrap().iter().enumerate() {
            let (sell, kind, key) = (step[0] == "sell", step[1].as_str().unwrap(), step[2].as_str().unwrap());
            let (amount, quote) = (Num::Dec(dec(&step[3])), Num::Dec(dec(&step[4])));
            let branch = if step[5] == true {
                books.unpriced_sell(&mut ledger, key, &amount, &quote).unwrap();
                "unpriced_sell"
            } else {
                let fill = Fill::of(sell, kind);
                books.apply(&mut ledger, fill, key, &amount, &quote).unwrap();
                match fill {
                    Fill::RebalanceSell => "sell", Fill::LiquidationSell => "liquidation_sell", Fill::RegularSell => "regular_sell",
                    Fill::RebalanceBuy => "rebalance_buy", Fill::RedeployBuy => "redeploy_buy", Fill::RegularBuy => "regular_buy",
                }
            };
            assert_eq!(branch, step[6].as_str().unwrap(), "step {i} of {steps}");
            *branches.entry(branch).or_insert(0) += 1;
            assert_eq!(tagged(&books.uninvested_cash().unwrap()), step[7], "uninvested cash after step {i}: {step}");
            let got = serde_json::json!({
                "basis": tagged(&books.basis), "cash": tagged(&books.cash), "contributed": tagged(&books.contributed), "realised_cash": tagged(&books.realised_cash),
                "realised_pnl": tagged(&books.realised_pnl), "estimated_cash": tagged(&books.estimated_cash), "divested": tagged(&books.divested),
            });
            assert_eq!(got, step[8], "books after step {i}: {step}");
            let entries: Vec<Value> = ledger.0.iter().map(|(key, entry)| serde_json::json!([key, tagged(&entry.amount), tagged(&entry.invested)])).collect();
            assert_eq!(Value::Array(entries), step[9], "ledger after step {i}: {step}");
        }
    }
    assert_eq!(branches.len(), 7, "every branch is met: {branches:?}");
}

// ---- the walk: how a row is read, a split ratio, the profit ratio, and the pin of the Ruby being mirrored ----

use deltabadger::figures::db::Order;
use deltabadger::figures::{splits, walk};

#[test]
fn an_order_row_is_read_as_filled_only_when_it_is_closed() {
    let v = vectors();
    let cases = v["confirmed_exec_amounts"].as_array().unwrap();
    assert_eq!(cases.len(), 216);
    for c in cases {
        let order = Order {
            id: 1, at: At(0), exchange_id: None, price: opt_dec(&c[1]), amount: opt_dec(&c[2]), amount_exec: opt_dec(&c[3]), quote_amount_exec: opt_dec(&c[4]),
            base: None, asset_id: None, sell: false, buy: true, closed: c[0] == "closed", kind: "REGULAR".into(),
        };
        let (amount, quote) = walk::confirmed_exec_amounts(&order).unwrap();
        assert_eq!((amount.map(|d| d.to_s_f()), quote.map(|d| d.to_s_f())), (c[5].as_str().map(str::to_string), c[6].as_str().map(str::to_string)), "{c}");
    }
}

#[test]
fn a_split_ratio_is_read_as_rails_reads_it_or_not_at_all() {
    let v = vectors();
    let cases = v["split_factor"].as_array().unwrap();
    assert_eq!(cases.len(), 36);
    for c in cases {
        let got = splits::factor(&serde_json::json!({ "split_ratio": c[0] })).unwrap().map(|f| f.to_s_f());
        assert_eq!(got.as_deref(), c[1].as_str(), "{c}");
    }
    assert_eq!(splits::factor(&serde_json::json!({})), Ok(None));
}

#[test]
fn the_profit_ratio_is_rubys_float_or_bigdecimal_as_rails_computes_it() {
    let v = vectors();
    let cases = v["pnl"].as_array().unwrap();
    assert_eq!(cases.len(), 8);
    for c in cases { assert_eq!(tagged(&walk::pnl(&num(&c[0]), &num(&c[1])).unwrap()), c[2], "{c}"); }
}

/// The Ruby this library mirrors, by content: a change to any of these files fails here until the port is checked
/// against it and the vectors are recorded again (script/rust/record_figures_vectors.rb).
#[test]
fn the_ruby_being_mirrored_has_not_changed() {
    use sha2::{Digest, Sha256};
    let v = vectors();
    let pinned = v["ported_sources"].as_object().unwrap();
    assert_eq!(pinned.len(), 12);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, sum) in pinned {
        let bytes = std::fs::read(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(hex::encode(Sha256::digest(&bytes)), sum.as_str().unwrap(), "{path} changed since the vectors were recorded");
    }
    assert_eq!(v["account_transaction_adjustment"], deltabadger::figures::db::ADJUSTMENT, "AccountTransaction.entry_types[:adjustment]");
    assert_eq!(v["versions"]["bigdecimal"], "3.3.1", "BigDecimal's division precision is the gem's: record again and re-run the grid when it moves");
}

// ---- Bot::ChartSeries: the timeframe, reading a grid, thinning the buy marks, marking a chart at market ----

use deltabadger::figures::chart::{self, BuyMark};
use deltabadger::figures::walk::Chart;

#[test]
fn the_candle_timeframe_follows_the_bots_age_as_rails_picks_it() {
    let v = vectors();
    let frames = v["timeframes"].as_array().unwrap();
    assert_eq!(frames.len(), 17);
    for c in frames { assert_eq!(chart::timeframe(float(&c[0])), c[1].as_i64().unwrap(), "{c}"); }
}

fn marks(v: &Value) -> Vec<(At, Dec)> { v.as_array().unwrap().iter().map(|m| (At(m[0].as_i64().unwrap()), dec(&m[1]))).collect() }

#[test]
fn a_price_grid_is_read_between_its_marks_as_rails_reads_it() {
    let v = vectors();
    let cases = v["grid_price"].as_array().unwrap();
    assert_eq!(cases.len(), 120);
    let (mut asked, mut read, mut between) = (0, 0, 0);
    for c in cases {
        let grid = marks(&c[0]);
        for q in c[1].as_array().unwrap() {
            let at = At(q[0].as_i64().unwrap());
            let got = chart::grid_price(&grid, at).unwrap().map(|p| p.to_s_f());
            assert_eq!(got.as_deref(), q[1].as_str(), "{q} in {}", c[0]);
            asked += 1;
            if got.is_some() { read += 1; }
            if got.is_some() && !grid.iter().any(|m| m.0 == at) { between += 1; }
        }
    }
    assert!(read > 150 && between > 60 && asked - read > 100, "{asked} asked, {read} read, {between} between two marks");
}

#[test]
fn buy_marks_are_thinned_and_summed_as_rails_thins_them() {
    let v = vectors();
    let cases = v["thinned_marks"].as_array().unwrap();
    assert_eq!(cases.len(), 11);
    let list = |v: &Value| -> Vec<BuyMark> {
        v.as_array().unwrap().iter().map(|m| BuyMark { at: At(m[0].as_i64().unwrap()), key: m[1].as_str().unwrap().into(), amount: dec(&m[2]), quote: dec(&m[3]), fills: m[4].as_i64().unwrap() }).collect()
    };
    let mut thinned = 0;
    for c in cases {
        let (given, want) = (list(&c[0]), list(&c[1]));
        if want.len() < given.len() { thinned += 1; }
        assert_eq!(chart::thinned_marks(given).unwrap(), want, "{} marks", c[0].as_array().unwrap().len());
    }
    assert_eq!(thinned, 6, "six histories are long enough to thin");
}

#[test]
fn a_chart_is_marked_at_market_point_for_point_as_rails_marks_it() {
    let v = vectors();
    let cases = v["marked_at_market"].as_array().unwrap();
    assert_eq!(cases.len(), 60);
    let rows = |v: &Value| -> Vec<Vec<(String, Num)>> {
        v.as_array().unwrap().iter().map(|row| row.as_array().unwrap().iter().map(|p| (p[0].as_str().unwrap().to_string(), num(&p[1]))).collect()).collect()
    };
    let nums = |v: &Value| -> Vec<Num> { v.as_array().unwrap().iter().map(num).collect() };
    let grids = |v: &Value| -> chart::Grids { v.as_array().unwrap().iter().map(|g| (g[0].as_str().unwrap().to_string(), marks(&g[1]))).collect() };
    let (mut kept, mut dropped, mut unpriced) = (0, 0, 0);
    for c in cases {
        let given = Chart {
            labels: c["labels"].as_array().unwrap().iter().map(|t| At(t.as_i64().unwrap())).collect(), value: nums(&c["values"]), invested: nums(&c["invested"]),
            extra: rows(&c["extra"]), invested_by: rows(&c["cost"]), cash: nums(&c["cash"]), prices: None, assets: None,
        };
        let priceable: Vec<String> = c["priceable"].as_array().unwrap().iter().map(|k| k.as_str().unwrap().to_string()).collect();
        let got = chart::marked_at_market(&given, &grids(&c["grids"]), &grids(&c["display"]), &priceable).unwrap();
        let out = serde_json::json!({
            "labels": got.labels.iter().map(|at| at.0).collect::<Vec<_>>(),
            "values": got.value.iter().map(tagged).collect::<Vec<_>>(),
            "invested": got.invested.iter().map(tagged).collect::<Vec<_>>(),
            "prices": got.prices.iter().flatten().map(|(key, serie)| serde_json::json!([key, serie.iter().map(|p| p.as_ref().map(|p| p.to_d().unwrap().to_s_f())).collect::<Vec<_>>()])).collect::<Vec<_>>(),
            "assets": got.assets.iter().flatten().map(|(key, serie)| serde_json::json!([key, serie.value.iter().map(|v| v.as_ref().map(tagged)).collect::<Vec<_>>(), serie.invested.iter().map(tagged).collect::<Vec<_>>()])).collect::<Vec<_>>(),
        });
        assert_eq!(out, c["out"], "{}", c);
        assert_eq!((&got.extra, &got.invested_by, &got.cash), (&given.extra, &given.invested_by, &given.cash), "the walk's own rows are carried unchanged");
        kept += got.labels.len();
        dropped += c["grids"].as_array().unwrap().iter().flat_map(|g| g[1].as_array().unwrap()).count();
        unpriced += got.assets.iter().flatten().flat_map(|(_, serie)| &serie.value).filter(|v| v.is_none()).count();
    }
    assert!(kept > 200 && dropped > kept / 2 && unpriced > 20, "{kept} points kept of {dropped} grid marks, {unpriced} left on their fill mark");
}
