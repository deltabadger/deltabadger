//! A28-1 (sync::fills): a tracker ledger row read back as the venue fill it records. Raw objects are the shapes each
//! importer stores (the ledger tests' venue answers: tests/*_continuation.rs, tests/coinbase.rs, tests/sync.rs), with the
//! venue's order and fill ids its documented answer carries. Rows the importer builds itself are built through it
//! (binance_ledger::normalize_trade, coinbase_ledger::fill, activities::normalize). No Rails, no network.
use chrono::{DateTime, Utc};
use deltabadger::ruby::BigDec;
use deltabadger::sync::activities::{CryptoPairs, Entry, Raw};
use deltabadger::sync::fills::{read, supported, Fill, LedgerRow, Read};
use deltabadger::venue::Side;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

const BINANCE: &str = "Exchanges::Binance";
const BINANCE_US: &str = "Exchanges::BinanceUs";
const COINBASE: &str = "Exchanges::Coinbase";
const BITGET: &str = "Exchanges::Bitget";
const BYBIT: &str = "Exchanges::Bybit";
const MEXC: &str = "Exchanges::Mexc";
const GEMINI: &str = "Exchanges::Gemini";
const BITVAVO: &str = "Exchanges::Bitvavo";
const BINGX: &str = "Exchanges::Bingx";
const BITRUE: &str = "Exchanges::Bitrue";
const HYPERLIQUID: &str = "Exchanges::Hyperliquid";
const ALPACA: &str = "Exchanges::Alpaca";
const T: i64 = 1_788_000_000_000;

const ASSETS: [(i64, &str, &str); 9] = [(1, "BTC", "Cryptocurrency"), (2, "USDT", "Cryptocurrency"), (3, "ETH", "Cryptocurrency"), (4, "USD", "Currency"),
    (5, "EUR", "Currency"), (6, "USDC", "Cryptocurrency"), (7, "HYPE", "Cryptocurrency"), (8, "AAPL", "Stock"), (9, "PURR", "Cryptocurrency")];

fn db() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    deltabadger::store::create_schema(&c).unwrap();
    for (id, symbol, category) in ASSETS {
        c.execute("INSERT INTO assets (id, external_id, symbol, category, created_at, updated_at) VALUES (?1, ?2, ?2, ?3, '2026-01-01', '2026-01-01')", params![id, symbol, category]).unwrap();
    }
    c
}
fn asset(symbol: &str) -> i64 { ASSETS.iter().find(|a| a.1 == symbol).unwrap().0 }
fn exchange(c: &Connection, ty: &str) -> i64 {
    c.execute("INSERT INTO exchanges (type, name, created_at, updated_at) VALUES (?1, ?1, '2026-01-01', '2026-01-01')", [ty]).unwrap();
    c.last_insert_rowid()
}
fn ticker(c: &Connection, exchange_id: i64, ticker: &str, base: &str, quote: &str) -> i64 {
    c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
               minimum_base_size, minimum_quote_size, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 8, 8, 2, 0, 0, '2026-01-01', '2026-01-01')",
              params![exchange_id, ticker, base, quote, asset(base), asset(quote)]).unwrap();
    c.last_insert_rowid()
}
fn d(s: &str) -> BigDec { BigDec::parse(s).unwrap() }
fn at(ms: i64) -> DateTime<Utc> { DateTime::from_timestamp_millis(ms).unwrap() }
fn row(exchange_id: i64, tx_id: Option<&str>, entry_type: i64, raw: Value) -> LedgerRow {
    LedgerRow { id: 1, tx_id: tx_id.map(str::to_string), entry_type, raw_data: raw, transacted_at: at(T), exchange_id }
}
fn stored(exchange_id: i64, e: &Entry) -> LedgerRow {
    LedgerRow { id: 1, tx_id: e.tx_id.clone(), entry_type: e.entry_type, raw_data: e.raw.value.clone(), transacted_at: e.transacted_at.unwrap(), exchange_id }
}
fn fill(c: &Connection, venue: &str, r: &LedgerRow) -> Fill {
    match read(c, venue, r).unwrap() { Read::Fill(f) => f, other => panic!("{venue}: expected a fill, got {other:?}") }
}
fn unreadable(c: &Connection, venue: &str, r: &LedgerRow) {
    assert!(matches!(read(c, venue, r).unwrap(), Read::Unreadable(_)), "{venue}: expected unreadable, got {:?}", read(c, venue, r));
}
/// (side, base, quote) of a fill.
fn trade(f: &Fill) -> (Side, BigDec, BigDec) { (f.side, f.base_qty.clone(), f.quote_qty.clone()) }

#[test]
fn supported_venues() {
    for v in [BINANCE, BINANCE_US, COINBASE, BITGET, BYBIT, MEXC, GEMINI, BITVAVO, BINGX, BITRUE, HYPERLIQUID, ALPACA] { assert!(supported(v), "{v}"); }
    for v in ["Exchanges::Kraken", "Exchanges::Kucoin", "Exchanges::KucoinHf", "Exchanges::Ibkr", "Exchanges::Nope"] { assert!(!supported(v), "{v}"); }
    let c = db();
    assert!(read(&c, "Exchanges::Kraken", &row(1, None, 0, json!({}))).is_err(), "an unsupported venue is the caller's error");
}

#[test]
fn non_trade_rows_are_not_trades() {
    let c = db();
    let ex = exchange(&c, BYBIT);
    for entry_type in [4, 5, 6, 8, 10, 11, 15, 16] { assert_eq!(read(&c, BYBIT, &row(ex, Some("x"), entry_type, json!({}))).unwrap(), Read::NotATrade); }
}

fn binance_raw(id: i64, order: i64, buyer: bool, qty: &str, quote: &str, ms: i64) -> Raw {
    Raw::from_value(json!({ "symbol": "ETHBTC", "id": id, "orderId": order, "orderListId": -1, "price": "0.05", "qty": qty, "quoteQty": quote,
                            "commission": "0.0001", "commissionAsset": "BNB", "time": ms, "isBuyer": buyer, "isMaker": false, "isBestMatch": true }))
}

#[test]
fn binance_fills() {
    for venue in [BINANCE, BINANCE_US] {
        let c = db();
        let ex = exchange(&c, venue);
        let eth_btc = ticker(&c, ex, "ETHBTC", "ETH", "BTC");
        ticker(&c, ex, "ETHUSDT", "ETH", "USDT");
        // A crypto-quoted buy: two swap rows of one fill, one Fill.
        let legs = deltabadger::sync::binance_ledger::normalize_trade(&binance_raw(7, 3, true, "2", "0.1", T), "ETHBTC", "ETH", "BTC").unwrap();
        assert_eq!((legs.len(), legs[0].entry_type, legs[1].entry_type), (2, 2, 3));
        let (a, b) = (fill(&c, venue, &stored(ex, &legs[0])), fill(&c, venue, &stored(ex, &legs[1])));
        assert_eq!(a, b, "both swap legs read as one fill");
        assert_eq!((a.fill_id.as_str(), a.ticker_id, a.base_asset_id, a.quote_asset_id), ("ETHBTC-3-7", eth_btc, asset("ETH"), asset("BTC")));
        // binance.rs:513 `format!("{}-{id}", o.pair)`; Rails binance.rb:279 splits the same "SYMBOL-orderId".
        assert_eq!(a.order_key, "ETHBTC-3");
        assert_eq!(trade(&a), (Side::Buy, d("2"), d("0.1")));
        assert_eq!(Some(a.at), legs[0].transacted_at, "the importer's own time");
        // One leg alone still reads (the same fill).
        assert_eq!(fill(&c, venue, &stored(ex, &legs[1])), a);
        // A crypto-quoted sell, a second fill of order 3: its own fill id, the same order.
        let sold = deltabadger::sync::binance_ledger::normalize_trade(&binance_raw(8, 3, false, "1", "0.05", T + 1), "ETHBTC", "ETH", "BTC").unwrap();
        let s = fill(&c, venue, &stored(ex, &sold[0]));
        assert_eq!((s.fill_id.as_str(), s.order_key.as_str(), trade(&s)), ("ETHBTC-3-8", "ETHBTC-3", (Side::Sell, d("1"), d("0.05"))));
        // A fiat-like quoted buy and sell: one row each.
        let mut raw = binance_raw(7, 3, true, "0.5", "1500", T);
        raw.set("symbol", json!("ETHUSDT"));
        let buy = deltabadger::sync::binance_ledger::normalize_trade(&raw, "ETHUSDT", "ETH", "USDT").unwrap();
        let f = fill(&c, venue, &stored(ex, &buy[0]));
        assert_eq!((buy.len(), f.fill_id.as_str(), f.order_key.as_str(), trade(&f)), (1, "ETHUSDT-3-7", "ETHUSDT-3", (Side::Buy, d("0.5"), d("1500"))));
        assert_ne!(f.fill_id, a.fill_id, "trade id 7 of order 3 on two pairs: two fills");
        raw.set("isBuyer", json!(false));
        let sell = deltabadger::sync::binance_ledger::normalize_trade(&raw, "ETHUSDT", "ETH", "USDT").unwrap();
        assert_eq!(fill(&c, venue, &stored(ex, &sell[0])).side, Side::Sell);
        // Malformed: an unreadable quantity, a tx_id that names another order, a missing isBuyer.
        let mut bad = stored(ex, &buy[0]);
        bad.raw_data["qty"] = json!("abc");
        unreadable(&c, venue, &bad);
        unreadable(&c, venue, &LedgerRow { tx_id: Some("ETHUSDT-4-7".into()), ..stored(ex, &buy[0]) });
        let mut no_side = stored(ex, &buy[0]);
        no_side.raw_data.as_object_mut().unwrap().remove("isBuyer");
        unreadable(&c, venue, &no_side);
        // Unmapped symbol.
        let mut raw = binance_raw(9, 4, true, "1", "1", T);
        raw.set("symbol", json!("SOLBTC"));
        let unmapped = deltabadger::sync::binance_ledger::normalize_trade(&raw, "SOLBTC", "SOL", "BTC").unwrap();
        unreadable(&c, venue, &stored(ex, &unmapped[0]));
        // A convert, a fiat payment, a dust conversion: no spot order. A file import's trade row: unreadable.
        assert_eq!(read(&c, venue, &row(ex, Some("q1-out"), 3, json!({ "quoteId": "q1", "fromAsset": "BTC", "toAsset": "ETH" }))).unwrap(), Read::NotATrade);
        assert_eq!(read(&c, venue, &row(ex, Some("fiat-buy-n1"), 0, json!({ "orderNo": "n1", "fiatCurrency": "EUR" }))).unwrap(), Read::NotATrade);
        assert_eq!(read(&c, venue, &row(ex, Some("dust-1-in"), 2, json!({ "transId": 1, "fromAsset": "XRP", "amount": "1", "transferedAmount": "0.001" }))).unwrap(), Read::NotATrade);
        unreadable(&c, venue, &row(ex, None, 0, json!({ "operation": "Buy" })));
    }
}

fn coinbase_raw(product: &str, trade_id: &str, order: &str, side: &str, size: &str, price: &str, in_quote: bool) -> Raw {
    Raw::from_value(json!({ "entry_id": format!("e{trade_id}"), "trade_id": trade_id, "order_id": order, "trade_time": "2026-09-19T01:00:00Z", "trade_type": "FILL",
                            "price": price, "size": size, "commission": "0.01", "product_id": product, "sequence_timestamp": "2026-09-19T01:00:00.1Z",
                            "liquidity_indicator": "TAKER", "size_in_quote": in_quote, "user_id": "u", "side": side, "retail_portfolio_id": "p" }))
}

#[test]
fn coinbase_fills() {
    let c = db();
    let ex = exchange(&c, COINBASE);
    let eth_eur = ticker(&c, ex, "ETH-EUR", "ETH", "EUR");
    ticker(&c, ex, "ETH-BTC", "ETH", "BTC");
    let one = |raw: Raw| deltabadger::sync::coinbase_ledger::fill(raw).unwrap();
    // A quote-sized buy (size is the quote) and a second fill of the same order.
    let a = one(coinbase_raw("ETH-EUR", "t1", "o-1", "BUY", "100", "2000", true));
    let b = one(coinbase_raw("ETH-EUR", "t2", "o-1", "BUY", "0.01", "2000", false));
    let (fa, fb) = (fill(&c, COINBASE, &stored(ex, &a[0])), fill(&c, COINBASE, &stored(ex, &b[0])));
    assert_eq!((fa.ticker_id, fa.fill_id.as_str(), trade(&fa)), (eth_eur, "ETH-EUR|t1", (Side::Buy, d("0.05"), d("100"))));
    assert_eq!((fb.fill_id.as_str(), trade(&fb)), ("ETH-EUR|t2", (Side::Buy, d("0.01"), d("20"))));
    // coinbase.rs:407 the raw `success_response.order_id`; Rails coinbase.rb:264 looks it up as stored.
    assert_eq!((fa.order_key.as_str(), fb.order_key.as_str()), ("o-1", "o-1"));
    assert_eq!(Some(fa.at), a[0].transacted_at);
    let sell = one(coinbase_raw("ETH-EUR", "t3", "o-2", "SELL", "0.5", "2000", false));
    assert_eq!(trade(&fill(&c, COINBASE, &stored(ex, &sell[0]))), (Side::Sell, d("0.5"), d("1000")));
    // A crypto-quoted fill: two swap rows, one Fill; one leg alone reads the same.
    let legs = one(coinbase_raw("ETH-BTC", "t4", "o-3", "SELL", "2", "0.05", false));
    assert_eq!((legs[0].entry_type, legs[1].entry_type), (3, 2));
    let (l0, l1) = (fill(&c, COINBASE, &stored(ex, &legs[0])), fill(&c, COINBASE, &stored(ex, &legs[1])));
    assert_eq!(l0, l1);
    assert_eq!((l0.fill_id.as_str(), trade(&l0)), ("ETH-BTC|t4", (Side::Sell, d("2"), d("0.1"))));
    // An adjusted fill: its entry_id is its identity, kept apart from the fill it adjusts.
    let mut adj = coinbase_raw("ETH-EUR", "t1", "o-1", "BUY", "0.01", "2000", false);
    adj.set("trade_type", json!("ADJUSTMENT"));
    adj.set("entry_id", json!("adj1"));
    let adjusted = one(adj);
    let fadj = fill(&c, COINBASE, &stored(ex, &adjusted[0]));
    assert_eq!((fadj.fill_id.as_str(), fadj.order_key.as_str()), ("ETH-EUR|adj1", "o-1"));
    assert_ne!(fadj.fill_id, fa.fill_id);
    // Malformed: no order_id, an unreadable time, a tx_id that is not the fill's. Unmapped product.
    let mut bad = stored(ex, &a[0]);
    bad.raw_data.as_object_mut().unwrap().remove("order_id");
    unreadable(&c, COINBASE, &bad);
    let mut bad = stored(ex, &a[0]);
    bad.raw_data["trade_time"] = json!("yesterday");
    unreadable(&c, COINBASE, &bad);
    unreadable(&c, COINBASE, &LedgerRow { tx_id: Some("t9".into()), ..stored(ex, &a[0]) });
    unreadable(&c, COINBASE, &stored(ex, &one(coinbase_raw("SOL-EUR", "t5", "o-4", "BUY", "1", "1", false))[0]));
}

/// tests/bitget_continuation.rs's fill, with Bitget's `amount` on the first; tx_id = `tradeId` (bitget_ledger::fill).
fn bitget_raw(trade: &str, order: &str, side: &str) -> Value {
    json!({ "symbol": "BTCUSDT", "orderId": order, "tradeId": trade, "side": side, "orderType": "market", "priceAvg": "60000", "size": "0.001",
            "feeDetail": { "feeCoin": "USDT", "totalFee": "-0.06" }, "cTime": T.to_string() })
}

#[test]
fn bitget_fills() {
    let c = db();
    let ex = exchange(&c, BITGET);
    let t = ticker(&c, ex, "BTCUSDT", "BTC", "USDT");
    let f1 = fill(&c, BITGET, &row(ex, Some("100"), 0, bitget_raw("100", "o1", "buy")));
    let mut with_amount = bitget_raw("101", "o1", "buy");
    with_amount["amount"] = json!("60.5");
    let f2 = fill(&c, BITGET, &row(ex, Some("101"), 0, with_amount));
    assert_eq!((f1.ticker_id, f1.fill_id.as_str(), trade(&f1), f1.at), (t, "BTCUSDT|100", (Side::Buy, d("0.001"), d("60")), at(T)));
    assert_eq!((f2.fill_id.as_str(), f2.quote_qty.clone()), ("BTCUSDT|101", d("60.5")));
    // bitget.rs:356 `format!("{}-{id}", o.pair)`.
    assert_eq!((f1.order_key.as_str(), f2.order_key.as_str()), ("BTCUSDT-o1", "BTCUSDT-o1"));
    assert_eq!(fill(&c, BITGET, &row(ex, Some("102"), 1, bitget_raw("102", "o2", "sell"))).side, Side::Sell);
    // A fill without a tradeId (bitget_continuation's bare fill) has no identity: unreadable.
    let mut bare = bitget_raw("x", "o3", "buy");
    bare.as_object_mut().unwrap().remove("tradeId");
    unreadable(&c, BITGET, &row(ex, None, 0, bare));
    let mut bad = bitget_raw("103", "o3", "buy");
    bad["size"] = json!("");
    unreadable(&c, BITGET, &row(ex, Some("103"), 0, bad));
    let mut other = bitget_raw("104", "o4", "buy");
    other["symbol"] = json!("ETHUSDT");
    unreadable(&c, BITGET, &row(ex, Some("104"), 0, other));
}

/// tests/bybit_continuation.rs's execution, with its ids; tx_id = `execId` (bybit_ledger::fill).
fn bybit_raw(exec: &str, order: &str, side: &str, qty: &str, value: &str) -> Value {
    json!({ "symbol": "BTCUSDT", "orderId": order, "orderLinkId": "", "execId": exec, "side": side, "execQty": qty, "execValue": value, "execPrice": "60000",
            "execFee": "-0.06", "feeCurrency": "USDT", "execTime": T.to_string() })
}

#[test]
fn bybit_fills() {
    let c = db();
    let ex = exchange(&c, BYBIT);
    let t = ticker(&c, ex, "BTCUSDT", "BTC", "USDT");
    let f1 = fill(&c, BYBIT, &row(ex, Some("e1"), 0, bybit_raw("e1", "1900", "Buy", "0.001", "60")));
    let f2 = fill(&c, BYBIT, &row(ex, Some("e2"), 0, bybit_raw("e2", "1900", "Buy", "0.002", "120")));
    assert_eq!((f1.ticker_id, f1.fill_id.as_str(), trade(&f1), f1.at), (t, "BTCUSDT|e1", (Side::Buy, d("0.001"), d("60")), at(T)));
    // bybit.rs:266 the bare `result.orderId`; Rails bybit.rb:291 #get_order finds it as stored.
    assert_eq!((f1.order_key.as_str(), f2.order_key.as_str(), f2.fill_id.as_str()), ("1900", "1900", "BTCUSDT|e2"));
    assert_eq!(trade(&fill(&c, BYBIT, &row(ex, Some("e3"), 1, bybit_raw("e3", "1901", "Sell", "0.001", "61")))), (Side::Sell, d("0.001"), d("61")));
    unreadable(&c, BYBIT, &row(ex, Some("e4"), 0, bybit_raw("e4", "1902", "Buy", "0.001", "lots")));
    unreadable(&c, BYBIT, &row(ex, Some("e5"), 0, bybit_raw("e5", "1902", "Both", "0.001", "60")));
    let mut other = bybit_raw("e6", "1903", "Buy", "1", "1");
    other["symbol"] = json!("DOGEUSDT");
    unreadable(&c, BYBIT, &row(ex, Some("e6"), 0, other));
}

/// tests/mexc_continuation.rs / bitrue_continuation.rs's trade; tx_id "SYMBOL-orderId-id" (mexc_ledger::trade, bitrue_ledger::trade).
fn spot_raw(symbol: &str, id: i64, order: &str, buyer: bool, quote: Option<&str>) -> Value {
    let mut v = json!({ "symbol": symbol, "id": id, "orderId": order, "price": "60000", "qty": "0.001", "commission": "-0.01", "commissionAsset": "USDT",
                        "time": T, "isBuyer": buyer });
    if let Some(q) = quote { v["quoteQty"] = json!(q); }
    v
}

#[test]
fn mexc_and_bitrue_fills() {
    for venue in [MEXC, BITRUE] {
        let c = db();
        let ex = exchange(&c, venue);
        let t = ticker(&c, ex, "BTCUSDT", "BTC", "USDT");
        ticker(&c, ex, "ETHUSDT", "ETH", "USDT");
        let order = if venue == MEXC { "C02__1" } else { "77" };
        let r = |symbol: &str, id: i64, buyer: bool| row(ex, Some(&format!("{symbol}-{order}-{id}")), if buyer { 0 } else { 1 }, spot_raw(symbol, id, order, buyer, Some("60")));
        let (f1, f2) = (fill(&c, venue, &r("BTCUSDT", 5, true)), fill(&c, venue, &r("BTCUSDT", 6, true)));
        assert_eq!((f1.ticker_id, f1.fill_id.clone(), trade(&f1), f1.at), (t, format!("BTCUSDT-{order}-5"), (Side::Buy, d("0.001"), d("60")), at(T)));
        // mexc.rs:289 / bitrue.rs:263 `format!("{}-{id}", o.pair)`; Rails mexc.rb:285 / bitrue.rb:286 split it back.
        assert_eq!((f1.order_key.clone(), f2.order_key.clone()), (format!("BTCUSDT-{order}"), format!("BTCUSDT-{order}")));
        assert_ne!(f1.fill_id, f2.fill_id);
        assert_eq!(fill(&c, venue, &r("BTCUSDT", 7, false)).side, Side::Sell);
        assert_ne!(fill(&c, venue, &r("ETHUSDT", 5, true)).fill_id, f1.fill_id, "trade id 5 on two pairs: two fills");
        // Malformed: no isBuyer, a tx_id of another order. Unmapped symbol.
        let mut bad = r("BTCUSDT", 8, true);
        bad.raw_data.as_object_mut().unwrap().remove("isBuyer");
        unreadable(&c, venue, &bad);
        unreadable(&c, venue, &LedgerRow { tx_id: Some("BTCUSDT-999-5".into()), ..r("BTCUSDT", 5, true) });
        unreadable(&c, venue, &r("SOLUSDT", 9, true));
        unreadable(&c, venue, &row(ex, None, 0, json!({})));
    }
    // Bitrue without a quoteQty: qty × price, as bitrue_ledger::trade.
    let c = db();
    let ex = exchange(&c, BITRUE);
    ticker(&c, ex, "BTCUSDT", "BTC", "USDT");
    assert_eq!(fill(&c, BITRUE, &row(ex, Some("BTCUSDT-77-1"), 0, spot_raw("BTCUSDT", 1, "77", true, None))).quote_qty, d("60"));
    unreadable(&c, MEXC, &row(exchange(&c, MEXC), Some("BTCUSDT-77-1"), 0, spot_raw("BTCUSDT", 1, "77", true, None)));
}

/// Gemini mytrades; tx_id = `tid` (gemini_ledger::trade), the symbol lowercase.
fn gemini_raw(tid: i64, order: &str, kind: &str) -> Value {
    json!({ "price": "60000.00", "amount": "0.001", "timestamp": T / 1000, "timestampms": T, "type": kind, "aggressor": true, "fee_currency": "USD",
            "fee_amount": "0.06", "tid": tid, "order_id": order, "exchange": "gemini", "is_auction_fill": false, "symbol": "btcusd" })
}

#[test]
fn gemini_fills() {
    let c = db();
    let ex = exchange(&c, GEMINI);
    let t = ticker(&c, ex, "BTCUSD", "BTC", "USD");
    let (f1, f2) = (fill(&c, GEMINI, &row(ex, Some("11"), 0, gemini_raw(11, "555", "Buy"))), fill(&c, GEMINI, &row(ex, Some("12"), 0, gemini_raw(12, "555", "Buy"))));
    assert_eq!((f1.ticker_id, f1.fill_id.as_str(), trade(&f1), f1.at), (t, "btcusd|11", (Side::Buy, d("0.001"), d("60")), at(T)));
    // gemini.rs:265 the raw `order_id`; Rails gemini.rb:275 #order_status by it.
    assert_eq!((f1.order_key.as_str(), f2.order_key.as_str(), f2.fill_id.as_str()), ("555", "555", "btcusd|12"));
    assert_eq!(fill(&c, GEMINI, &row(ex, Some("13"), 1, gemini_raw(13, "556", "Sell"))).side, Side::Sell);
    let mut bad = gemini_raw(14, "557", "Buy");
    bad["price"] = json!(null);
    unreadable(&c, GEMINI, &row(ex, Some("14"), 0, bad));
    let mut other = gemini_raw(15, "558", "Buy");
    other["symbol"] = json!("ethusd");
    unreadable(&c, GEMINI, &row(ex, Some("15"), 0, other));
}

/// Bitvavo /trades; tx_id = `id` (bitvavo_ledger::trade).
fn bitvavo_raw(id: &str, order: &str, side: &str) -> Value {
    json!({ "id": id, "orderId": order, "timestamp": T, "market": "BTC-EUR", "side": side, "amount": "0.002", "price": "50000", "taker": true,
            "fee": "0.25", "feeCurrency": "EUR", "settled": true })
}

#[test]
fn bitvavo_fills() {
    let c = db();
    let ex = exchange(&c, BITVAVO);
    let t = ticker(&c, ex, "BTC-EUR", "BTC", "EUR");
    let (f1, f2) = (fill(&c, BITVAVO, &row(ex, Some("tr1"), 0, bitvavo_raw("tr1", "ord-1", "buy"))), fill(&c, BITVAVO, &row(ex, Some("tr2"), 0, bitvavo_raw("tr2", "ord-1", "buy"))));
    assert_eq!((f1.ticker_id, f1.fill_id.as_str(), trade(&f1), f1.at), (t, "BTC-EUR|tr1", (Side::Buy, d("0.002"), d("100")), at(T)));
    // bitvavo.rs:359 `format!("{market}-{id}")`; Rails bitvavo.rb:490 splits it back.
    assert_eq!((f1.order_key.as_str(), f2.order_key.as_str(), f2.fill_id.as_str()), ("BTC-EUR-ord-1", "BTC-EUR-ord-1", "BTC-EUR|tr2"));
    assert_eq!(fill(&c, BITVAVO, &row(ex, Some("tr3"), 1, bitvavo_raw("tr3", "ord-2", "sell"))).side, Side::Sell);
    let mut bad = bitvavo_raw("tr4", "ord-3", "buy");
    bad.as_object_mut().unwrap().remove("market");
    unreadable(&c, BITVAVO, &row(ex, Some("tr4"), 0, bad));
    let mut other = bitvavo_raw("tr5", "ord-4", "buy");
    other["market"] = json!("ETH-EUR");
    unreadable(&c, BITVAVO, &row(ex, Some("tr5"), 0, other));
}

/// tests/bingx_continuation.rs's trade; tx_id = the bare `id` (bingx_ledger::trade).
fn bingx_raw(symbol: &str, id: &str, order: &str, buyer: bool) -> Value {
    json!({ "symbol": symbol, "id": id, "orderId": order, "quoteQty": "60", "price": "60000", "qty": "0.001", "commission": "-0.01",
            "commissionAsset": "USDT", "time": T, "isBuyer": buyer })
}

#[test]
fn bingx_fills() {
    let c = db();
    let ex = exchange(&c, BINGX);
    let t = ticker(&c, ex, "BTC-USDT", "BTC", "USDT");
    ticker(&c, ex, "ETH-USDT", "ETH", "USDT");
    let (f1, f2) = (fill(&c, BINGX, &row(ex, Some("1"), 0, bingx_raw("BTC-USDT", "1", "9", true))), fill(&c, BINGX, &row(ex, Some("2"), 0, bingx_raw("BTC-USDT", "2", "9", true))));
    assert_eq!((f1.ticker_id, f1.fill_id.as_str(), trade(&f1), f1.at), (t, "BTC-USDT|9|1", (Side::Buy, d("0.001"), d("60")), at(T)));
    // bingx.rs:463 `format!("{}-{id}", o.pair)`; Rails bingx.rb:714 rpartitions it back.
    assert_eq!((f1.order_key.as_str(), f2.order_key.as_str()), ("BTC-USDT-9", "BTC-USDT-9"));
    assert_ne!(f1.fill_id, f2.fill_id);
    assert_ne!(fill(&c, BINGX, &row(ex, Some("1"), 0, bingx_raw("ETH-USDT", "1", "9", true))).fill_id, f1.fill_id, "trade id 1 on two pairs: two fills");
    assert_eq!(fill(&c, BINGX, &row(ex, Some("3"), 1, bingx_raw("BTC-USDT", "3", "10", false))).side, Side::Sell);
    unreadable(&c, BINGX, &row(ex, Some("4"), 0, json!({ "id": "4", "orderId": "11", "qty": "1", "quoteQty": "1", "time": T, "isBuyer": true })));
    unreadable(&c, BINGX, &row(ex, Some("other"), 0, bingx_raw("BTC-USDT", "5", "12", true)));
    unreadable(&c, BINGX, &row(ex, Some("6"), 0, bingx_raw("SOL-USDT", "6", "13", true)));
}

/// tests/hyperliquid_continuation.rs's spot fill; tx_id = `tid` (hyperliquid_ledger::fill).
fn hyperliquid_raw(coin: &str, tid: i64, oid: i64, side: &str) -> Value {
    json!({ "coin": coin, "px": "40", "sz": "2.5", "side": side, "time": T, "hash": "0x1", "oid": oid, "fee": "0.01", "tid": tid, "feeToken": "USDC" })
}

#[test]
fn hyperliquid_fills() {
    let c = db();
    let ex = exchange(&c, HYPERLIQUID);
    let t = ticker(&c, ex, "@107", "HYPE", "USDC");
    ticker(&c, ex, "PURR/USDC", "PURR", "USDC");
    let (f1, f2) = (fill(&c, HYPERLIQUID, &row(ex, Some("1"), 0, hyperliquid_raw("@107", 1, 70, "B"))), fill(&c, HYPERLIQUID, &row(ex, Some("2"), 0, hyperliquid_raw("@107", 2, 70, "B"))));
    assert_eq!((f1.ticker_id, f1.base_asset_id, f1.fill_id.as_str(), trade(&f1), f1.at), (t, asset("HYPE"), "@107|1", (Side::Buy, d("2.5"), d("100")), at(T)));
    // hyperliquid.rs:418 `format!("{base}-{oid}")` (the ticker's base); Rails hyperliquid.rb:524 #build_order_id(ticker.base, oid).
    assert_eq!((f1.order_key.as_str(), f2.order_key.as_str(), f2.fill_id.as_str()), ("HYPE-70", "HYPE-70", "@107|2"));
    let purr = fill(&c, HYPERLIQUID, &row(ex, Some("3"), 1, hyperliquid_raw("PURR/USDC", 3, 71, "A")));
    assert_eq!((purr.side, purr.order_key.as_str()), (Side::Sell, "PURR-71"));
    // Unmapped coin, a perp coin, a malformed size.
    unreadable(&c, HYPERLIQUID, &row(ex, Some("4"), 0, hyperliquid_raw("@999", 4, 72, "B")));
    unreadable(&c, HYPERLIQUID, &row(ex, Some("5"), 0, hyperliquid_raw("BTC", 5, 73, "B")));
    let mut bad = hyperliquid_raw("@107", 6, 74, "B");
    bad["sz"] = json!("1,5");
    unreadable(&c, HYPERLIQUID, &row(ex, Some("6"), 0, bad));
    // A colliding coin (two tickers named "@107": the schema's unique index forbids it; dropped here to prove the guard).
    c.execute_batch("DROP INDEX index_exchange_tickers_on_unique_ticker; DROP INDEX index_exchange_tickers_on_unique_base_asset_and_quote_asset").unwrap();
    ticker(&c, ex, "@107", "PURR", "USDC");
    assert_eq!(read(&c, HYPERLIQUID, &row(ex, Some("1"), 0, hyperliquid_raw("@107", 1, 70, "B"))).unwrap(), Read::Unreadable("symbol names several tickers".into()));
}

/// An Alpaca FILL activity (tests/sync.rs's, with the `order_id` Alpaca's trade activity carries), through activities::normalize.
fn alpaca_entry(symbol: &str, id: &str, order: &str, side: &str) -> Entry {
    let raw = Raw::from_value(json!({ "id": id, "activity_type": "FILL", "transaction_time": "2026-09-10T14:30:00Z", "type": "fill", "price": "200", "qty": "0.5",
                                       "side": side, "symbol": symbol, "leaves_qty": "0", "order_id": order, "cum_qty": "0.5", "order_status": "filled" }));
    deltabadger::sync::activities::normalize(&raw, &CryptoPairs::default()).unwrap().unwrap()
}

#[test]
fn alpaca_fills() {
    let c = db();
    let ex = exchange(&c, ALPACA);
    let aapl = ticker(&c, ex, "AAPL", "AAPL", "USD");
    let btc = ticker(&c, ex, "BTC/USD", "BTC", "USD");
    let (f1, f2) = (fill(&c, ALPACA, &stored(ex, &alpaca_entry("AAPL", "a1", "ord-uuid-1", "buy"))), fill(&c, ALPACA, &stored(ex, &alpaca_entry("AAPL", "a2", "ord-uuid-1", "buy"))));
    assert_eq!((f1.ticker_id, f1.fill_id.as_str(), trade(&f1)), (aapl, "AAPL|a1", (Side::Buy, d("0.5"), d("100"))));
    assert_eq!(f1.at, "2026-09-10T14:30:00Z".parse::<DateTime<Utc>>().unwrap());
    // alpaca.rs:306 the order's raw `id`; Rails alpaca.rb:1007 `order_id: result.data['id']`.
    assert_eq!((f1.order_key.as_str(), f2.order_key.as_str(), f2.fill_id.as_str()), ("ord-uuid-1", "ord-uuid-1", "AAPL|a2"));
    assert_eq!(fill(&c, ALPACA, &stored(ex, &alpaca_entry("AAPL", "a3", "ord-uuid-2", "sell"))).side, Side::Sell);
    // Crypto: compact ("BTCUSD", activities' crypto pairs) or slashed, the same ticker.
    assert_eq!(fill(&c, ALPACA, &stored(ex, &alpaca_entry("BTCUSD", "a4", "ord-uuid-3", "buy"))).ticker_id, btc);
    assert_eq!(fill(&c, ALPACA, &stored(ex, &alpaca_entry("BTC/USD", "a5", "ord-uuid-3", "buy"))).ticker_id, btc);
    // Malformed: no order_id. Unmapped symbol. A non-FILL activity on a trade row.
    let mut bad = stored(ex, &alpaca_entry("AAPL", "a6", "x", "buy"));
    bad.raw_data.as_object_mut().unwrap().remove("order_id");
    unreadable(&c, ALPACA, &bad);
    unreadable(&c, ALPACA, &stored(ex, &alpaca_entry("TSLA", "a7", "ord-uuid-4", "buy")));
    unreadable(&c, ALPACA, &row(ex, Some("a8"), 0, json!({ "id": "a8", "activity_type": "DIV" })));
}
