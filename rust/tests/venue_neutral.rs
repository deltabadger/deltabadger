mod common;
use common::seed::{self, BotSpec, TxSpec};
use chrono::Duration;
use deltabadger::crypto::Credentials;
use deltabadger::engine::model::{self, Ticker};
use deltabadger::engine::run::{self, Engine};
use deltabadger::engine::venue_rules::KRAKEN;
use deltabadger::engine::{amount, placement, polling, FixedClock, SteppingClock};
use deltabadger::lease;
use deltabadger::ruby::BigDec;
use deltabadger::store::{self, Paths};
use deltabadger::venue::fake::FakeVenue;
use deltabadger::venue::{OrderState, OrderStatus, PriceSide, Venue, VenueError, VenueFactory};
use std::cell::RefCell;
use std::rc::Rc;

fn xbteur() -> Ticker {
    Ticker { id: 1, ticker: "XBTEUR".into(), base_code: "XBT".into(), quote_code: "EUR".into(), base_symbol: "BTC".into(), quote_symbol: "EUR".into(), exchange_name: "Kraken".into(),
             base_asset_id: 1, quote_asset_id: 2, base_decimals: 8, quote_decimals: 5, price_decimals: 1,
             minimum_base_size: BigDec::parse("0.00005").unwrap(), minimum_quote_size: BigDec::parse("0.5").unwrap(), trading_enabled: true, available: true, crypto: true, }
}

#[tokio::test(flavor = "current_thread")]
async fn a_zero_kraken_book_is_refused_with_rails_message_naming_the_pair() {
    let v = FakeVenue::new().ticker("XXBTZEUR", "0", "0", "0");
    assert_eq!(v.price(&xbteur(), PriceSide::Ask).await, Err(VenueError::Rejected(vec!["Wrong ask price for XBTEUR: 0.0".into()])));
    assert_eq!(v.price(&xbteur(), PriceSide::Last).await, Err(VenueError::Rejected(vec!["Wrong last price for XBTEUR: 0.0".into()])));
    assert!(std::ptr::eq(v.rules(), &KRAKEN));
}

#[test]
fn a_failed_status_changes_nothing() {
    let (_d, o, s) = common::install();
    let bot = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let tx = seed::insert_tx(&o.primary, &s, bot, &TxSpec { status: 0, external_status: Some(0), external_id: Some("O1".into()), order_type: 0,
        amount: None, quote_amount: Some("60"), price: Some("50000"), quote_amount_exec: None, amount_exec: None, created_at: "2026-09-01 10:00:01".into() });
    let row = || -> String { o.primary.query_row("SELECT json_array(status, external_status, price, amount_exec, updated_at) FROM transactions WHERE id = ?1", [tx], |r| r.get(0)).unwrap() };
    let before = row();
    let state = OrderState { asset_class: None, txid: "O1".into(), status: OrderStatus::Failed, price: Some(BigDec::zero()), amount: None, quote_amount: None,
                             amount_exec: BigDec::zero(), quote_amount_exec: BigDec::zero(), limit: false, sell: false, pair: None };
    { let unit=o.primary.unchecked_transaction().unwrap(); let fenced=deltabadger::engine::model::check_credential_result(&unit,&None).unwrap();
    polling::apply_in(&fenced, bot, tx, &state, true, "2026-09-01T10:00:06Z".parse().unwrap()).unwrap();
    unit.commit().unwrap(); }
    assert_eq!(row(), before, "Rails' poll jobs have no branch for :failed");
}

#[derive(Clone)]
struct Recording(FakeVenue, Rc<RefCell<Vec<String>>>);
impl VenueFactory for Recording {
    type V = FakeVenue;
    fn for_bot(&self, exchange_type: &str, _credentials: Option<Credentials>) -> FakeVenue { self.1.borrow_mut().push(exchange_type.into()); self.0.clone() }
}

#[tokio::test(flavor = "current_thread")]
async fn the_factory_is_asked_for_each_bots_exchange_type() {
    let dir = common::rails_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let lock = lease::lock(&p, "2026-09-01T00:00:00Z".parse().unwrap()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let seen = Rc::new(RefCell::new(vec![]));
    let v = FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "100000", "0");
    let mut e = Engine::new(o.primary, Recording(v.clone(), seen.clone()), seed::cipher(), lock);
    run::step(&mut e, &FixedClock("2026-09-01T10:00:00.5Z".parse().unwrap())).await.unwrap();
    assert_eq!(v.sent().len(), 1);
    assert!(!seen.borrow().is_empty() && seen.borrow().iter().all(|t| t == "Exchanges::Kraken"), "{:?}", seen.borrow());
}

#[tokio::test(flavor = "current_thread")]
async fn an_intent_delayed_before_its_send_is_never_sent_and_is_settled_as_not_placed() {
    let (_d, o, s) = common::install();
    let bot = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"))).unwrap();
    let ticker = model::ticker_for(&o.primary, &bot).unwrap().unwrap();
    let amount::Sizing::Place(plan) = amount::size(&bot, &ticker, &BigDec::from_i64(60), &BigDec::from_i64(50_000), KRAKEN.minimum_logic).unwrap() else { panic!() };
    let t0: chrono::DateTime<chrono::Utc> = "2026-09-30T12:00:00Z".parse().unwrap();
    let intent = placement::begin(&o.primary, &bot, &plan, &FixedClock(t0)).unwrap();
    let v = FakeVenue::new();
    // The process stalls 11 s between committing the intent and sending it.
    let late = SteppingClock::new(t0 + Duration::seconds(11), Duration::seconds(1));
    assert!(matches!(placement::send(&v, &intent, &late).await, placement::Sent::NotSent(_)));
    assert!(v.sent().is_empty(), "a stale intent never reaches the venue");
    let bot = model::load_bot(&o.primary, bot.id).unwrap();
    assert!(matches!(placement::recover(&o.primary, &v, &bot, &FixedClock(t0 + Duration::seconds(70))).await.unwrap(), placement::Recovery::NotPlaced));
}

#[tokio::test(flavor = "current_thread")]
async fn a_kraken_insufficient_funds_rejection_stamps_the_budget_too() {
    let (_d, o, s) = common::install();
    let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let v = FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "100000", "0")
        .next_add(deltabadger::venue::fake::AddOutcome::Reject(vec!["EOrder:Insufficient funds".into()]));
    deltabadger::engine::tick::tick(&o.primary, &v, id, &FixedClock("2026-09-01T10:00:00.5Z".parse().unwrap()), &mut Default::default()).await.unwrap();
    let stamped: bool = o.primary.query_row("SELECT last_end_of_funds_notification IS NOT NULL FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert!(stamped, "Bot::Failable#record_failure! (not ported yet)");
}
