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
