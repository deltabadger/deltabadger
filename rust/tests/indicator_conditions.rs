mod common;
use common::seed::{self, BotSpec};
use deltabadger::engine::{conditions::{self, Rule, Evaluated, Checked}, model, FixedClock};
use deltabadger::engine::tick::PriceCache;
use deltabadger::engine::candles::Candle;
use deltabadger::ruby::BigDec;
use deltabadger::venue::{fake::FakeVenue};
use serde_json::json;

fn candles(value: i64) -> Vec<Candle> {
    let at = "2026-09-02T00:00:00Z".parse::<chrono::DateTime<chrono::Utc>>().unwrap();
    (1..=30).rev().map(|i| Candle { at: at - chrono::Duration::days(i), open: BigDec::from_i64(value),
        high: BigDec::from_i64(value), low: BigDec::from_i64(value), close: BigDec::from_i64(value), volume: BigDec::from_i64(1) }).collect()
}

#[test]
fn readers_use_rails_defaults_and_refuse_invalid_values_by_name() {
    let (_d, o, s) = common::install();
    for selling in [false, true] {
        for (kind, prefix) in conditions::TRIGGERS.into_iter().filter(|(k, _)| *k != Rule::Price) {
            let prefix = format!("{}{prefix}", if selling { "sell_" } else { "" });
            let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with(&format!("{prefix}ed"), json!(true)));
            let mut bot = model::load_bot(&o.primary, id).unwrap();
            let rule = bot.condition_rule(kind, selling).unwrap().unwrap();
            assert_eq!(rule.common.value_condition.as_deref(), Some(if selling { "above" } else { "below" }));
            assert_eq!(rule.common.after, kind == Rule::PriceDrop);
            use deltabadger::engine::{candles::Timeframe, technical::{MaType, Window}};
            assert!(match rule.reading {
                conditions::Reading::Drop(w) => w == if selling { Window::TwentyFourHours } else { Window::Ath },
                conditions::Reading::Ma(tf, period, ma) => (tf, period, ma) == (Timeframe::Day, 9, MaType::Sma),
                conditions::Reading::Rsi(tf) => tf == Timeframe::Day && rule.common.limit.to_dec().unwrap() == BigDec::from_i64(if selling { 70 } else { 30 }),
                _ => false,
            });
            let field = match kind { Rule::PriceDrop => "_time_window_condition", Rule::MovingAverage => "_in_ma_type", _ => "_in_indicator" };
            bot.settings[format!("{prefix}{field}")] = json!("invalid");
            assert!(bot.condition_rule(kind, selling).unwrap_err().contains(&format!("{prefix}{field}")));
        }
    }
}

#[test]
fn malformed_condition_values_are_named_and_legacy_sell_ath_is_a_recent_low() {
    let (_d, o, s) = common::install();
    for (kind, field, value) in [
        (Rule::PriceDrop, "price_drop_limit", json!(1.1)),
        (Rule::PriceDrop, "price_drop_limit_time_window_condition", json!("seven_days")),
        (Rule::MovingAverage, "moving_average_limit_in_period", json!(0)),
        (Rule::MovingAverage, "moving_average_limit_in_timeframe", json!("one_minute")),
        (Rule::Indicator, "indicator_limit", json!("30")),
        (Rule::Indicator, "indicator_limit_value_condition", json!("between")),
        (Rule::Indicator, "indicator_limit_timing_condition", json!("sometimes")),
    ] {
        let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with(&format!("{}ed", kind.prefix(false)), json!(true)).with(field, value));
        assert!(model::load_bot(&o.primary, id).unwrap().condition_rule(kind, false).unwrap_err().contains(field));
    }
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("sell_price_drop_limited", json!(true)).with("sell_price_drop_limit_time_window_condition", json!("ath")));
    assert!(matches!(model::load_bot(&o.primary, id).unwrap().condition_rule(Rule::PriceDrop, true).unwrap().unwrap().reading,
        conditions::Reading::Drop(deltabadger::engine::technical::Window::TwentyFourHours)));
}

#[tokio::test(flavor = "current_thread")]
async fn outer_unmet_rule_is_the_only_rule_read_and_latches_need_no_reads() {
    let (_d, o, s) = common::install();
    let mut spec = BotSpec::weekly(60.0, "2026-09-01 10:00:00");
    for (_, prefix) in conditions::TRIGGERS { spec = spec.with(&format!("{prefix}ed"), json!(true)); }
    let id = seed::insert_bot(&o.primary, &s, &spec);
    let mut bot = model::load_bot(&o.primary, id).unwrap();
    let version = model::credential_version(&o.primary, &bot).unwrap();
    let mut series = candles(100);
    for (i, candle) in series.iter_mut().enumerate() { candle.close = BigDec::from_i64(100 + (i % 2) as i64); }
    let venue = FakeVenue::new().candles_answer(Ok(series));
    let clock = FixedClock("2026-09-02T10:00:00Z".parse().unwrap());
    let (kind, answer) = conditions::gate(&o.primary, &venue, &bot, false, &clock, &PriceCache::default(), &version).await.unwrap();
    assert_eq!(kind, Rule::Indicator);
    assert!(matches!(answer, Evaluated::Unmet));
    assert_eq!(venue.candle_reads().len(), 1);
    for (_, prefix) in conditions::TRIGGERS {
        bot.settings[format!("{prefix}_timing_condition")] = json!("after");
        bot.transient[format!("{prefix}_condition_met_at")] = json!("2026-09-01T10:00:00Z");
    }
    let (_, answer) = conditions::gate(&o.primary, &venue, &bot, false, &clock, &PriceCache::default(), &version).await.unwrap();
    assert!(matches!(answer, Evaluated::Met { .. }));
    assert_eq!(venue.candle_reads().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn each_inner_gate_waits_for_the_outer_latches_and_drop_nil_is_unmet() {
    let (_d, o, s) = common::install();
    for (outer, expected) in [(1, Rule::MovingAverage), (2, Rule::PriceDrop), (3, Rule::Price)] {
        let mut spec = BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("price_limit", json!(1))
            .with("price_drop_limit_time_window_condition", json!("twenty_four_hours"));
        for (i, kind) in [Rule::Indicator, Rule::MovingAverage, Rule::PriceDrop, Rule::Price].into_iter().enumerate() {
            let prefix = kind.prefix(false);
            spec = spec.with(&format!("{prefix}ed"), json!(true)).with(&format!("{prefix}_timing_condition"), json!("after"));
            if i < outer { spec = spec.transient(&format!("{prefix}_condition_met_at"), json!("2026-09-01T10:00:00Z")); }
        }
        let id = seed::insert_bot(&o.primary, &s, &spec);
        let bot = model::load_bot(&o.primary, id).unwrap();
        let version = model::credential_version(&o.primary, &bot).unwrap();
        let venue = FakeVenue::new().ticker("XXBTZEUR", "50000", "50000", "50000")
            .candles_answer(Ok(if outer == 2 { vec![] } else { candles(40000) }));
        let (kind, answer) = conditions::gate(&o.primary, &venue, &bot, false, &FixedClock("2026-09-02T10:00:00Z".parse().unwrap()), &PriceCache::default(), &version).await.unwrap();
        assert_eq!(kind, expected);
        assert!(matches!(answer, Evaluated::Unmet));
        assert_eq!(venue.candle_reads().len(), usize::from(outer < 3));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn candle_info_never_stamps_or_clears_trading_state() {
    let (_d, o, s) = common::install();
    for kind in [Rule::PriceDrop, Rule::MovingAverage, Rule::Indicator] {
        let prefix = kind.prefix(false);
        let id = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") }
            .with(&format!("{prefix}ed"), json!(true)).with("price_drop_limit_time_window_condition", json!("twenty_four_hours"))
            .with("indicator_limit", json!(101)).transient("missed_quote_amount", json!("37")));
        let bot = model::load_bot(&o.primary, id).unwrap();
        let mut series = candles(100000);
        if kind == Rule::Indicator {
            for (i, candle) in series.iter_mut().enumerate() { candle.close = BigDec::from_i64(100000 + (i % 2) as i64); }
        }
        let venue = FakeVenue::new().ticker("XXBTZEUR", "50000", "50000", "50000").candles_answer(Ok(series));
        let now = "2026-09-02T10:00:00Z".parse().unwrap();
        conditions::want_info(&o.primary, id, None, prefix, None, "en").unwrap();
        let claim = conditions::info_claims(&o.primary, prefix).unwrap().remove(0);
        let answer = conditions::info_rule(&o.primary, &venue, &claim, kind, &FixedClock(now), &PriceCache::default(), &model::credential_version(&o.primary, &bot).unwrap()).await.unwrap();
        assert_eq!(answer.unwrap()["condition_met"], json!(true));
        let after = model::load_bot(&o.primary, id).unwrap();
        assert!(after.transient.get(format!("{prefix}_condition_met_at").as_str()).is_none());
        assert_eq!(after.transient["missed_quote_amount"], json!("37"));
        assert_eq!(after.status, bot.status);
        assert_eq!(after.settings, bot.settings);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn paused_index_checks_only_its_logged_rule_and_rearms() {
    let (_d, o, s) = common::install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")
        .with("indicator_limited", json!(true)).with("moving_average_limited", json!(true))
        .with("moving_average_limit_timing_condition", json!("after"))
        .transient("moving_average_limit_condition_met_at", json!("2026-09-01T10:00:00Z")));
    o.primary.execute("UPDATE bots SET type = 'Bots::DcaIndex', status = 6 WHERE id = ?1", [id]).unwrap();
    let now = "2026-09-02T10:00:00Z".parse().unwrap();
    model::log_activity(&o.primary, id, "limit_paused", model::Level::Info, json!({"limit_type":"moving_average"}), now).unwrap();
    let bot = model::load_bot(&o.primary, id).unwrap();
    assert_eq!(model::condition_paused_type(&o.primary, &bot).unwrap(), Some(Rule::MovingAverage));
    let version = model::credential_version(&o.primary, &bot).unwrap();
    assert_eq!(conditions::check(&o.primary, &FakeVenue::new(), &bot, &FixedClock(now), &PriceCache::default(), &version).await.unwrap(), Checked::Rearmed);
}

#[tokio::test(flavor = "current_thread")]
async fn clearing_ma_keeps_the_carry_when_another_rule_already_closed_the_window() {
    let (_d, o, s) = common::install();
    for kind in ["Bots::DcaMultiAsset", "Bots::DcaIndex"] {
        let id = seed::insert_bot(&o.primary, &s, &BotSpec { status: 6, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") }
            .with("moving_average_limited", json!(true)).with("moving_average_limit_in_ticker_id", json!(s.ticker_id))
            .with("price_limited", json!(true)).transient("moving_average_limit_condition_met_at", json!("2026-09-01T10:00:00Z"))
            .transient("missed_quote_amount", json!("37")).transient("missed_quote_amount_was_set", json!(true)));
        o.primary.execute("UPDATE bots SET type = ?1 WHERE id = ?2", (kind, id)).unwrap();
        let now = "2026-09-02T10:00:00Z".parse().unwrap();
        model::log_activity(&o.primary, id, "limit_paused", model::Level::Info, json!({"limit_type":"moving_average"}), now).unwrap();
        let bot = model::load_bot(&o.primary, id).unwrap();
        let venue = FakeVenue::new().ticker("XXBTZEUR", "50000", "50000", "50000").candles_answer(Ok(candles(40000)));
        let answer = conditions::check(&o.primary, &venue, &bot, &FixedClock(now), &PriceCache::default(), &model::credential_version(&o.primary, &bot).unwrap()).await.unwrap();
        assert!(matches!(answer, Checked::Next(_)));
        let after = model::load_bot(&o.primary, id).unwrap();
        assert_eq!(after.transient["missed_quote_amount"], json!("37"));
        assert!(after.transient["missed_quote_amount_was_set"].is_null());
        assert!(after.transient["moving_average_limit_condition_met_at"].is_null());
    }
}

#[test]
fn check_and_repair_use_the_paused_rules_timeframe() {
    let (_d, o, s) = common::install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("indicator_limited", json!(true)));
    let mut bot = model::load_bot(&o.primary, id).unwrap();
    let now = "2026-09-30T23:50:00Z".parse().unwrap();
    for (tf, next, repair) in [("one_hour", "2026-10-01T00:00:00Z", "2026-10-01T01:00:00Z"),
        ("one_month", "2026-09-30T23:59:59.999999999Z", "2026-10-31T23:59:59.999999999Z"),
        ("one_week", "2026-10-04T23:59:59.999999999Z", "2026-10-04T23:59:59.999999999Z")] {
        bot.settings["indicator_limit_in_timeframe"] = json!(tf);
        assert_eq!(conditions::next_at(&bot, Rule::Indicator, now).unwrap(), next.parse::<chrono::DateTime<chrono::Utc>>().unwrap());
        assert_eq!(conditions::repair_at(&bot, Rule::Indicator, now).unwrap(), repair.parse::<chrono::DateTime<chrono::Utc>>().unwrap());
    }
}

/// Index and signal gates use the recorded basket decision; Rails does not include these concerns on those classes.
#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn index_and_signal_candle_gates_match_recorded_rails_basket_twins() {
    use serde_json::Value;
    use deltabadger::venue::{http::ScriptedTransport, kraken::KrakenVenue};
    let built = tempfile::tempdir().unwrap();
    common::decisions::built_grid("grid-conditions-indicators", built.path());
    let recorded = tempfile::tempdir().unwrap();
    let mut names = vec![];
    for entry in std::fs::read_dir(built.path()).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap();
        if !name.starts_with("indicator-") || name.contains("-check_") { continue; }
        common::decisions::copy_dir(&path, &recorded.path().join(name));
        names.push(name.to_string());
    }
    assert_eq!(names.len(), 144);
    common::decisions::rails(&["record", recorded.path().to_str().unwrap()]);
    for name in names {
        let rails: Value = serde_json::from_str(&std::fs::read_to_string(recorded.path().join(&name).join("rails.json")).unwrap()).unwrap();
        let scenario: Value = serde_json::from_str(&std::fs::read_to_string(built.path().join(&name).join("scenario.json")).unwrap()).unwrap();
        let selling = name.contains("-sell-");
        let kind = if name.contains("-price_drop-") { Rule::PriceDrop } else if name.contains("-moving_average-") { Rule::MovingAverage } else { Rule::Indicator };
        let key = format!("{}_condition_met_at", kind.prefix(selling));
        for bot_type in ["Bots::DcaIndex", "Bots::Signal"] {
            if selling && bot_type == "Bots::DcaIndex" { continue; }
            let scratch = tempfile::tempdir().unwrap();
            let dir = scratch.path().join("cell");
            common::decisions::copy_dir(&built.path().join(&name), &dir);
            let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
            deltabadger::store::test_support::migrate(&c).unwrap();
            let original = model::load_bot(&c, 1).unwrap();
            let base = original.asset_ids()[0];
            let ticker = model::ticker_for_asset(&c, &original, base).unwrap().unwrap();
            let mut settings = original.settings.clone();
            settings[format!("{}_in_ticker_id", kind.prefix(selling))] = json!(ticker.id);
            if bot_type == "Bots::Signal" { settings["base_asset_id"] = json!(base); }
            c.execute("UPDATE bots SET type = ?1, settings = ?2 WHERE id = 1", rusqlite::params![bot_type, settings.to_string()]).unwrap();
            let bot = model::load_bot(&c, 1).unwrap();
            let version = model::credential_version(&c, &bot).unwrap();
            let clock = FixedClock(scenario["at"].as_str().unwrap().parse().unwrap());
            let prices = PriceCache::default();
            macro_rules! gate {
                ($venue:expr) => { conditions::gate(&c, &$venue, &bot, selling, &clock, &prices, &version).await.unwrap() };
            }
            let (_, evaluated) = match scenario["venue"].as_str() {
                Some("alpaca") => {
                    use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
                    let http = ScriptedTransport::from_script(&scenario["script"]["alpaca"]);
                    http.set_now(clock.0);
                    gate!(AlpacaVenue::new(http, Urls::for_passphrase(Some("paper"))))
                }
                Some("binance") => {
                    use deltabadger::venue::binance::{BinanceVenue, Pairs};
                    let http = ScriptedTransport::from_script(&scenario["script"]["binance"]);
                    http.set_now(clock.0);
                    let pairs = Pairs::load(&c, bot.exchange_id).unwrap();
                    gate!(BinanceVenue::new(http, "https://api.binance.com", "s").with_pairs(pairs))
                }
                _ => {
                    let http = ScriptedTransport::from_script(&deltabadger::parity::kraken_script(&scenario["script"]));
                    http.set_now(clock.0);
                    gate!(KrakenVenue::new(http))
                }
            };
            assert_eq!(matches!(evaluated, Evaluated::Met { .. }), !rails["sent"].as_array().unwrap().is_empty(), "{name} {bot_type}");
            let expected = rails["changes"]["bots"].as_array().unwrap().iter().find(|b| b["id"] == 1)
                .map(|b| b["after"]["transient_data"][&key].clone()).unwrap_or_else(|| original.transient[&key].clone());
            assert_eq!(model::load_bot(&c, 1).unwrap().transient[&key], expected, "{name} {bot_type}: latch write");
            if bot_type == "Bots::Signal" { assert_eq!(model::load_bot(&c, 1).unwrap().transient["missed_quote_amount"], original.transient["missed_quote_amount"], "signal has no carry write"); }
        }
    }
}


#[test]
fn candle_status_names_the_selected_pair_and_preserves_the_recorded_number() {
    use deltabadger::web::bot::{Bot, For, settings::condition_info};
    let (_d, o, s) = common::install_alpaca();
    let (eth, _) = seed::add_eth_sol(&o.primary, &s);
    let spec = BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("allocations", json!({s.btc.to_string(): 0.5, eth.to_string(): 0.5}))
        .with("moving_average_limited", json!(true)).with("indicator_limited", json!(true))
        .with("moving_average_limit_in_ticker_id", json!(s.ticker_id)).with("indicator_limit_in_ticker_id", json!(s.ticker_id));
    let id = seed::insert_bot(&o.primary, &s, &spec);
    let bot = Bot::find(&o.primary, s.user_id, id, For::Page, "en").unwrap().unwrap();
    for (rule, value) in [("moving_average_limit", "123.45"), ("indicator_limit", "50.0")] {
        let html = condition_info("en", &bot, rule, false, &json!({"condition_met":true,"value":value}), "status");
        assert!(html.contains("BTCUSD candle close"), "{html}");
        assert!(html.contains(value) && html.contains("text-success"), "{html}");
    }
}


/// D-COND-LOOKBACK-RANGE: a valid positive period can exceed the date range of the candle reader.
#[tokio::test(flavor = "current_thread")]
async fn an_unrepresentable_lookback_fails_without_panicking_or_fetching_candles() {
    let (_d, o, s) = common::install();
    for period in [u64::from(u32::MAX), 1_000_000] {
        let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00")
            .with("moving_average_limited", json!(true)).with("moving_average_limit_in_period", json!(period))
            .with("moving_average_limit_in_timeframe", json!("one_month")));
        let bot = model::load_bot(&o.primary, id).unwrap();
        assert!(bot.condition_rule(Rule::MovingAverage, false).unwrap().is_some());
        let venue = FakeVenue::new().ticker("XXBTZEUR", "50000", "50000", "50000");
        let (_, answer) = conditions::gate(&o.primary, &venue, &bot, false, &FixedClock("2026-09-02T10:00:00Z".parse().unwrap()),
            &PriceCache::default(), &model::credential_version(&o.primary, &bot).unwrap()).await.unwrap();
        assert!(matches!(answer, Evaluated::Failed(_)));
        assert!(venue.candle_reads().is_empty());
    }
}
