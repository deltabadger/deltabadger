//! What Rails' Exchange subclasses decide differently per venue, as one table per venue selected by exchange type:
//! Exchanges::*::ERRORS with Exchange#failure_kind (substring, first kind wins, FAILURE_KINDS order), the polls'
//! #transient_error? / #throttled_error?, #minimum_amount_logic, the order wire format, and when an order the engine
//! sent can be proven absent.
use super::model::Ticker;
use crate::ruby::BigDec;
use crate::venue::Side;

/// One branch of Bot::OrderSetter#calculate_best_amount_info (order_setter.rb:23-77).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AmountLogic {
    /// :base_or_quote and :base_and_quote (one branch in Rails): quote or base, whichever minimum binds.
    BaseOrQuote,
    /// :base_and_quote_in_base: always base, against max(ceil(minimum_quote_size / price), minimum_base_size).
    BaseAndQuoteInBase,
    /// :quote, against minimum_quote_size only.
    Quote,
    /// :base, against minimum_base_size only.
    Base,
}

/// Exchange#minimum_amount_logic for a buy, by order type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MinimumLogic { pub market: AmountLogic, pub limit: AmountLogic }

/// How an OrderPlan becomes the strings the venue receives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WireFormat {
    /// Honeymaker's Kraken client: BigDecimal#to_s('F'); a market buy may be `viqc` (volume in quote).
    Kraken,
    /// Exchanges::Alpaca#set_market_order / #set_limit_order: #wire_decimal, the floored decimal at N places;
    /// a market buy is `notional`, a limit buy `qty` + `limit_price`.
    Alpaca,
    /// Exchanges::Bitget#set_market_order / #set_limit_order (G1, #532): BigDecimal#to_s('F') strings and one `size` in the
    /// unit the order takes: quote coins for a market buy, base coins for a market sell and a limit order. An amount in the
    /// other unit is converted at #market_price_for (the touch the order was sized at, floored to the tick) and floored to
    /// that unit's decimals, as Rails converts it.
    Bitget,
    /// Bybit takes either market unit; limit quantities are converted to base before the intent.
    Bybit,
}

/// Exchange#adjusted_price: how a price is put on the venue's tick before it is used or sent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PriceRule {
    /// Exchange#adjusted_price's default: `price_decimals` places.
    Decimals,
    /// Exchanges::Hyperliquid#adjusted_price: at most `figures` significant figures and at most
    /// `max_decimals - base_decimals` places (SIGNIFICANT_FIGURES 5, SPOT_MAX_DECIMALS 8); an integer is always valid.
    SignificantFigures { figures: i64, max_decimals: i64 },
}

/// Exchanges::Hyperliquid's rule (SIGNIFICANT_FIGURES 5, SPOT_MAX_DECIMALS 8), for its row (plan VB slice B1).
pub const HYPERLIQUID_PRICE_RULE: PriceRule = PriceRule::SignificantFigures { figures: 5, max_decimals: 8 };
/// Exchanges::Gemini::MARKET_CROSS and Exchanges::Hyperliquid::MARKET_CROSS, for their rows (slices B1, B3).
pub const MARKET_CROSS: &str = "0.01";

/// The `method:` of Ticker#adjusted_price.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rounding { Floor, Ceil, Round }

pub struct VenueRules {
    /// The exchanges.type STI name this table belongs to.
    pub exchange_type: &'static str,
    /// The venue as messages name it.
    pub name: &'static str,
    /// Exchange::FAILURE_KINDS order: insufficient_funds, invalid_key, permission_denied, restricted, throttle, transient.
    pub kinds: [(&'static str, &'static [&'static str]); 6],
    /// What the polls' Exchange#transient_error? matches, for this venue as the engine applies it.
    pub transient_text: &'static [&'static str],
    /// AddOrder answers that leave it unknown whether the order was placed (a sanctioned divergence: Rails writes a failed row).
    pub add_outcome_unknown: &'static [&'static str],
    pub minimum_logic: MinimumLogic,
    pub wire: WireFormat,
    /// Kraken's AddOrder carries the intent's `deadline`, after which Kraken drops the order: absence is provable from
    /// deadline + 60 s. A venue without one: reach_within_secs is `at` + the send window (placement::SEND_WINDOW_SECONDS,
    /// after which an intent is never sent) plus the client's whole request budget, the bound the transport enforces;
    /// absence there waits absence_margin_secs, which also covers the kernel's retransmissions.
    pub deadline_sent: bool,
    pub reach_within_secs: i64,
    /// A venue without a server-side deadline (deadline_sent false): absence is concluded only from a lookup that starts
    /// this long after BOTH the intent's `at` and this process's start. Unused where the deadline is sent.
    pub absence_margin_secs: i64,
    /// Rails' in-app clients (Clients::Alpaca) raise Client::TransientNetworkError on a transport failure; honeymaker's
    /// return a Failure. It decides what a failed price or balance read does to a tick.
    pub transport_raises: bool,
    /// Bot::FetchAndUpdateOrderJob raises on an unknown status and on a failed fetch. True for
    /// Alpaca (one GET per order, as Exchange#get_order); false keeps Kraken's follow-up exactly as merged.
    pub follow_up_strict: bool, pub market_hours: bool,
    /// Whether #get_orders names the ids it did not find (`missing`), so polling recovers them from trades and abandons stale
    /// ones (Bot::StaleOrderResolver). Exchanges::Binance#get_orders always answers `missing: []`: an id it did not return is
    /// simply left waiting.
    pub reports_missing: bool,
    /// Whether a lookup that finds nothing can ever prove an order absent (placement::recover_since's absence rule). False on
    /// KuCoin (plan K r4): its lookups can only FIND an order, so an unfound one waits for `deltabadger resolve-placement`.
    pub absence_provable: bool,
    /// Exchange#authoritative_missing_orders? false where #get_orders names missing ids: a young order the venue left out
    /// fails the sweep ("Exchange omitted recent order(s)"). Kraken is authoritative; Alpaca's and Binance's #get_orders
    /// never name one in Rails, so the guard cannot fire there.
    pub young_missing_fails: bool,
    /// Exchange#adjusted_price (plan VB slice P).
    pub price_rule: PriceRule,
    /// MARKET_CROSS of a venue whose #market_price_for crosses the book (Gemini, Hyperliquid: "0.01"), as decimal text.
    pub cross: Option<&'static str>,
}

/// BigDecimal#exponent of a non-zero value: the n with |x| = 0.d… × 10^n (66.6 → 2, 3.5 → 1, 0.0234 → -1). From the plain
/// text, as `ruby.rs` is a hash-pinned accounting primitive.
fn exponent(price: &BigDec) -> i64 {
    let text = price.to_s_f();
    let (whole, fraction) = text.trim_start_matches('-').split_once('.').unwrap_or(("0", ""));
    if whole != "0" { whole.len() as i64 } else { -((fraction.len() - fraction.trim_start_matches('0').len()) as i64) }
}

fn any(patterns: &[&str], messages: &[String]) -> bool { patterns.iter().any(|p| messages.iter().any(|m| m.contains(p))) }

impl VenueRules {
    pub fn failure_kind(&self, messages: &[String]) -> Option<&'static str> {
        self.kinds.iter().find(|(_, pats)| any(pats, messages)).map(|(k, _)| *k)
    }
    pub fn is_throttle(&self, messages: &[String]) -> bool { self.kinds.iter().any(|(k, pats)| *k == "throttle" && any(pats, messages)) }
    pub fn is_transient(&self, messages: &[String]) -> bool { any(self.transient_text, messages) }
    /// Exchange#invalid_key_error?'s string half: a message carrying one of `known_errors[:invalid_key]`.
    pub fn is_invalid_key(&self, messages: &[String]) -> bool { self.kinds.iter().any(|(k, pats)| *k == "invalid_key" && any(pats, messages)) }
    pub fn add_outcome_unknown(&self, messages: &[String]) -> bool { any(self.add_outcome_unknown, messages) }

    /// Ticker#adjusted_price → Exchange#adjusted_price. `Err` where Rails raises: a scale outside `ruby::MAX_SCALE`, or
    /// Hyperliquid's `clamp(0, max_decimals)` with a negative max (base_decimals above 8 raises ArgumentError).
    pub fn adjusted_price(&self, price: &BigDec, ticker: &Ticker, method: Rounding) -> Result<BigDec, String> {
        let decimals = match self.price_rule {
            PriceRule::Decimals => ticker.price_decimals,
            PriceRule::SignificantFigures { figures, max_decimals } => {
                if price.is_zero() { return Ok(price.clone()); }
                let max = max_decimals - ticker.base_decimals;
                if max < 0 { return Err(format!("ticker base_decimals {} leaves no price decimals", ticker.base_decimals)); }
                (figures - exponent(price)).clamp(0, max)
            }
        };
        let places = crate::ruby::scale(decimals).map_err(|_| format!("price decimals {decimals} (only 0..={})", crate::ruby::MAX_SCALE))?;
        Ok(match method { Rounding::Floor => price.floor(places), Rounding::Ceil => price.ceil(places), Rounding::Round => price.round(places) })
    }

    /// Exchange#market_price_for after its read of `touch` (the ask for a buy, the bid for a sell): adjusted_price(touch);
    /// on a crossing venue (Gemini#/Hyperliquid#market_price_for) adjusted AGAIN after × (1 ± cross), as Rails does:
    /// adjust(adjust(touch) × cross), not adjust(touch × cross).
    pub fn market_price(&self, touch: &BigDec, ticker: &Ticker, side: Side) -> Result<BigDec, String> {
        let adjusted = self.adjusted_price(touch, ticker, Rounding::Floor)?;
        let Some(cross) = self.cross else { return Ok(adjusted) };
        let cross = BigDec::parse(cross).map_err(|e| format!("{e:?}"))?;
        let multiplier = if side == Side::Buy { &BigDec::one() + &cross } else { &BigDec::one() - &cross };
        self.adjusted_price(&(&adjusted * &multiplier), ticker, Rounding::Floor)
    }
}

pub static KRAKEN: VenueRules = VenueRules {
    exchange_type: "Exchanges::Kraken",
    name: "Kraken",
    kinds: [
        ("insufficient_funds", &["EAPI:Insufficient funds", "EOrder:Insufficient funds"]),
        ("invalid_key", &["EAPI:Invalid key", "EAPI:Invalid signature"]),
        ("permission_denied", &["EGeneral:Permission denied"]),
        ("restricted", &["EAccount:Invalid permissions"]),
        ("throttle", &["EAPI:Rate limit exceeded"]),
        ("transient", &["EGeneral:Internal error", "EAPI:Invalid nonce", "EService:Unavailable", "EService:Busy", "EService:Deadline elapsed"]),
    ],
    transient_text: &["EGeneral:Internal error", "EAPI:Invalid nonce", "EService:Unavailable", "EService:Busy", "EService:Deadline elapsed"],
    // The transient kind minus "EAPI:Invalid nonce", which Kraken returns before it looks at the order.
    add_outcome_unknown: &["EGeneral:Internal error", "EService:Unavailable", "EService:Busy", "EService:Deadline elapsed"],
    // Exchanges::Kraken: a market buy is :base_or_quote, a limit buy :base.
    minimum_logic: MinimumLogic { market: AmountLogic::BaseOrQuote, limit: AmountLogic::Base },
    wire: WireFormat::Kraken,
    deadline_sent: true,
    reach_within_secs: 10,
    absence_margin_secs: 0, // Kraken enforces the deadline server-side: absence is deadline + 60 s (placement::recover_since)
    transport_raises: false,
    follow_up_strict: false, market_hours: false,
    reports_missing: true,
    absence_provable: true,
    young_missing_fails: false,
    price_rule: PriceRule::Decimals,
    cross: None,
};

/// Client::NETWORK_TRANSIENT_PATTERNS: Exchange#transient_error? applies them to every venue.
pub const NETWORK_TRANSIENT_PATTERNS: [&str; 12] = ["Net::ReadTimeout", "Net::OpenTimeout", "Faraday::TimeoutError", "Faraday::ConnectionFailed",
    "execution expired", "Connection reset", "Errno::ECONNRESET", "connection refused", "Connection refused", "Errno::ECONNREFUSED",
    "end of file reached", "unexpected eof while reading"];

pub static ALPACA: VenueRules = VenueRules {
    exchange_type: "Exchanges::Alpaca",
    name: "Alpaca",
    // Exchanges::Alpaca::ERRORS: only these two; a JSON "unauthorized." condemns, a synthesised "HTTP 401" does not.
    kinds: [("insufficient_funds", &["insufficient buying power"]), ("invalid_key", &["unauthorized"]), ("permission_denied", &[]),
            ("restricted", &[]), ("throttle", &[]), ("transient", &[])],
    transient_text: &NETWORK_TRANSIENT_PATTERNS,
    // A 5xx or a lost reply is ambiguous by HTTP status, decided in AlpacaVenue::add_order, never by text.
    add_outcome_unknown: &[],
    // Exchanges::Alpaca: always :quote.
    minimum_logic: MinimumLogic { market: AmountLogic::Quote, limit: AmountLogic::Quote },
    wire: WireFormat::Alpaca,
    deadline_sent: false,
    reach_within_secs: 55, // send window 10 s + request budget 45 s (connect 5 + write 10 + read 30)
    // 20 minutes, NOT reach_within_secs + 60. reqwest's timeout ends our wait, not the kernel's: bytes already handed to the
    // socket can still reach Alpaca after the client gave up. Linux defaults bound how long:
    // - an orphaned socket (closed by us, data unacknowledged) retransmits for tcp_orphan_retries, about 100 s;
    // - a socket still owned by a stalled or frozen process retransmits until tcp_retries2 = 15, about 924 s (15.4 min).
    // 1200 s clears both with margin. Do not tune it down: a shorter margin can declare "not placed" for an order that
    // later lands, and the next tick buys twice. Kraken is unaffected: its `deadline` is enforced server-side.
    absence_margin_secs: 1200,
    transport_raises: true,
    follow_up_strict: true, market_hours: true,
    reports_missing: true,
    absence_provable: true,
    young_missing_fails: false,
    price_rule: PriceRule::Decimals,
    cross: None,
};

/// Exchanges::Binance::ERRORS[:transient]: the -1021 strings (also Exchange::PLACEMENT_SAFE_TRANSIENT_ERRORS).
pub const BINANCE_TIMESTAMP_ERRORS: [&str; 2] = ["Timestamp for this request is outside of the recvWindow", "Timestamp for this request was"];
/// Exchange#transient_error? for Binance: Client::NETWORK_TRANSIENT_PATTERNS plus the venue's own transient strings.
const BINANCE_TRANSIENT: [&str; 14] = ["Net::ReadTimeout", "Net::OpenTimeout", "Faraday::TimeoutError", "Faraday::ConnectionFailed",
    "execution expired", "Connection reset", "Errno::ECONNRESET", "connection refused", "Connection refused", "Errno::ECONNREFUSED",
    "end of file reached", "unexpected eof while reading",
    "Timestamp for this request is outside of the recvWindow", "Timestamp for this request was"];

pub static BINANCE: VenueRules = BINANCE_RULES;
/// Exchanges::BinanceUs: Binance's own rules (#known_errors is Binance's ERRORS, every other rule inherited), under its
/// own type and name.
pub static BINANCE_US: VenueRules = VenueRules { exchange_type: "Exchanges::BinanceUs", name: "Binance.US", ..BINANCE_RULES };

const BINANCE_RULES: VenueRules = VenueRules {
    exchange_type: "Exchanges::Binance",
    name: "Binance",
    // Exchanges::Binance::ERRORS (binance.rb:3-9). Rails has no throttle list; the one text is venue::binance::HTTP_THROTTLE,
    // which only a read answered HTTP 429/418 carries (spec E §2.10, a listed divergence).
    kinds: [("insufficient_funds", &["Account has insufficient balance for requested action."]),
            ("invalid_key", &["API-key format invalid.", "Invalid API-key, IP, or permissions for action.", "Invalid Api-Key ID."]),
            ("permission_denied", &[]), ("restricted", &[]), ("throttle", &["Binance: request rate limit exceeded"]),
            ("transient", &BINANCE_TIMESTAMP_ERRORS)],
    transient_text: &BINANCE_TRANSIENT,
    // Exchange#ambiguous_placement_error? is decided by HTTP status and transport in BinanceVenue::add_order, never by text.
    add_outcome_unknown: &[],
    // Exchanges::Binance#minimum_amount_logic: market :base_and_quote (Rails runs the :base_or_quote branch), limit
    // :base_and_quote_in_base.
    minimum_logic: MinimumLogic { market: AmountLogic::BaseOrQuote, limit: AmountLogic::BaseAndQuoteInBase },
    // Honeymaker::Clients::Binance#new_order sends BigDecimal#to_s('F') strings, as Kraken's client does.
    wire: WireFormat::Kraken,
    // No server-side deadline is sent (recvWindow shortening is a separate, listed change): Alpaca's 20-minute margin.
    deadline_sent: false,
    reach_within_secs: 55,
    absence_margin_secs: 1200,
    transport_raises: false,
    // Bot::FetchAndUpdateOrderJob asks Exchanges::Binance#get_order (GET /api/v3/order): one order, strict.
    follow_up_strict: true, market_hours: false,
    reports_missing: false,
    absence_provable: true,
    young_missing_fails: false,
    price_rule: PriceRule::Decimals,
    cross: None,
};

/// Exchange#transient_error? for KuCoin: Client::NETWORK_TRANSIENT_PATTERNS plus Exchanges::Kucoin::ERRORS[:transient].
const KUCOIN_TRANSIENT: [&str; 15] = ["Net::ReadTimeout", "Net::OpenTimeout", "Faraday::TimeoutError", "Faraday::ConnectionFailed",
    "execution expired", "Connection reset", "Errno::ECONNRESET", "connection refused", "Connection refused", "Errno::ECONNREFUSED",
    "end of file reached", "unexpected eof while reading",
    "Invalid KC-API-TIMESTAMP", "Internal Server Error", "The system is busy"];

pub static KUCOIN: VenueRules = VenueRules {
    exchange_type: "Exchanges::Kucoin",
    name: "KuCoin",
    // Exchanges::Kucoin::ERRORS (kucoin.rb:3-28), pinned by tests/kucoin.rs against the recorded list.
    kinds: [("insufficient_funds", &["Balance insufficient!", "Insufficient balance"]),
            ("invalid_key", &["Invalid API-Key", "Invalid KC-API-SIGN", "Invalid passphrase", "The API key does not exist or site mismatch.",
                              "KC-API-KEY does not exist", "KC-API-KEY not exists", "KC-API-PASSPHRASE", "Signature error"]),
            ("permission_denied", &["Access Denied"]), ("restricted", &[]), ("throttle", &["429000", "Too Many Requests"]),
            ("transient", &["Invalid KC-API-TIMESTAMP", "Internal Server Error", "The system is busy"])],
    transient_text: &KUCOIN_TRANSIENT,
    // Plan K r5: the venue's outage and busy answers (500000, 230005) leave a placement unknown whatever the HTTP status, as
    // Exchange#ambiguous_placement_error? reads ERRORS[:transient]; "Invalid KC-API-TIMESTAMP" (400002) is placement-safe
    // instead. The network patterns are left to the transport, which knows whether the request left (Client.pre_transmission?).
    add_outcome_unknown: &["Internal Server Error", "The system is busy", "500000", "230005"],
    // Exchanges::Kucoin#minimum_amount_logic: market :base_or_quote, limit :base_and_quote_in_base (kucoin.rb:362).
    minimum_logic: MinimumLogic { market: AmountLogic::BaseOrQuote, limit: AmountLogic::BaseAndQuoteInBase },
    // Honeymaker::Clients::Kucoin#place_order sends BigDecimal#to_s('F') strings.
    wire: WireFormat::Kraken,
    deadline_sent: false,
    reach_within_secs: 55,
    absence_margin_secs: 1200,
    transport_raises: false,
    // Bot::FetchAndUpdateOrderJob asks Exchanges::Kucoin#get_order: one order, strict.
    follow_up_strict: true, market_hours: false,
    // #get_orders names a forgotten order under `missing` (kucoin.rb:312).
    reports_missing: true,
    absence_provable: false,
    young_missing_fails: true,
    price_rule: PriceRule::Decimals,
    cross: None,
};

/// Exchange#transient_error? for Bitvavo: Client::NETWORK_TRANSIENT_PATTERNS plus Exchanges::Bitvavo::ERRORS[:transient].
const BITVAVO_TRANSIENT: [&str; 20] = ["Net::ReadTimeout", "Net::OpenTimeout", "Faraday::TimeoutError", "Faraday::ConnectionFailed",
    "execution expired", "Connection reset", "Errno::ECONNRESET", "connection refused", "Connection refused", "Errno::ECONNREFUSED",
    "end of file reached", "unexpected eof while reading",
    "\"errorCode\":101", "\"errorCode\":107", "\"errorCode\":108", "\"errorCode\":109", "\"errorCode\":111", "\"errorCode\":400", "\"errorCode\":419", "\"errorCode\":304"];

pub static BITVAVO: VenueRules = VenueRules {
    exchange_type: "Exchanges::Bitvavo",
    name: "Bitvavo",
    // Exchanges::Bitvavo::ERRORS (bitvavo.rb:3-25), matched on the body as Bitvavo spells it.
    kinds: [("insufficient_funds", &["sufficient balance to complete"]),
            ("invalid_key", &["No active API key found", "API Key must be of length", "\"errorCode\":308", "\"errorCode\":309"]),
            ("permission_denied", &["\"errorCode\":307", "\"errorCode\":310", "\"errorCode\":311", "\"errorCode\":312"]),
            ("restricted", &[]), ("throttle", &["\"errorCode\":105", "Too Many Requests"]),
            ("transient", &["\"errorCode\":101", "\"errorCode\":107", "\"errorCode\":108", "\"errorCode\":109", "\"errorCode\":111", "\"errorCode\":400",
                            "\"errorCode\":419", "\"errorCode\":304"])],
    transient_text: &BITVAVO_TRANSIENT,
    // Exchange#ambiguous_placement_error?: ERRORS[:transient] leaves a placement unknown whatever the HTTP status, but 304,
    // placement-safe (an authentication failure before the matching engine: placement::PLACEMENT_SAFE_TRANSIENT_ERRORS).
    add_outcome_unknown: &["\"errorCode\":101", "\"errorCode\":107", "\"errorCode\":108", "\"errorCode\":109", "\"errorCode\":111", "\"errorCode\":400",
                           "\"errorCode\":419"],
    // Exchanges::Bitvavo#minimum_amount_logic: market :base_or_quote, limit :base_and_quote_in_base.
    minimum_logic: MinimumLogic { market: AmountLogic::BaseOrQuote, limit: AmountLogic::BaseAndQuoteInBase },
    // Honeymaker::Clients::Bitvavo#place_order sends BigDecimal#to_s('F') strings.
    wire: WireFormat::Kraken,
    deadline_sent: false,
    reach_within_secs: 55,
    absence_margin_secs: 1200,
    transport_raises: false,
    // Bot::FetchAndUpdateOrderJob asks #get_order, which recovers a gone order (240) from the trades and else names it not
    // found for Bot::StaleOrderResolver: the lenient poll (polling::poll_rows) of that one order, trades recovery included.
    follow_up_strict: false, market_hours: false,
    reports_missing: true,
    // Plan VB §1.5: the client-id lookup can only FIND an order; an unfound one waits for `deltabadger resolve-placement`.
    absence_provable: false,
    // #authoritative_missing_orders? true: the trades walk exhausts the fills, so a young order left out just waits.
    young_missing_fails: false,
    price_rule: PriceRule::Decimals,
    cross: None,
};

/// Exchange#transient_error? for Bitget: Client::NETWORK_TRANSIENT_PATTERNS plus Exchanges::Bitget::ERRORS[:transient].
const BITGET_TRANSIENT: [&str; 15] = ["Net::ReadTimeout", "Net::OpenTimeout", "Faraday::TimeoutError", "Faraday::ConnectionFailed",
    "execution expired", "Connection reset", "Errno::ECONNRESET", "connection refused", "Connection refused", "Errno::ECONNREFUSED",
    "end of file reached", "unexpected eof while reading",
    "Request timestamp expired", "Request timed out", "System is abnormal"];

pub static BITGET: VenueRules = VenueRules {
    exchange_type: "Exchanges::Bitget",
    name: "Bitget",
    // Exchanges::Bitget::ERRORS (bitget.rb:3-23, post-audit G4), matched inside the body or honeymaker's api_error text;
    // pinned by tests/bitget.rs against the recorded list.
    kinds: [("insufficient_funds", &["Insufficient balance", "Insufficient account balance"]),
            ("invalid_key", &["Invalid Api Key", "Invalid ACCESS_KEY", "Invalid signature", "Apikey does not exist", "sign signature error",
                              "apikey/password is incorrect", "passphrase is error", "User's ApiKey does not exist"]),
            ("permission_denied", &["Incorrect permissions", "The user does not have this permission"]), ("restricted", &[]),
            ("throttle", &["Too many requests", "Too Many Requests"]),
            ("transient", &["Request timestamp expired", "Request timed out", "System is abnormal"])],
    transient_text: &BITGET_TRANSIENT,
    // Exchange#ambiguous_placement_error?: ERRORS[:transient] leaves a placement unknown, but 40008 (placement-safe: the
    // signature check refused it before the book) is checked first. The network patterns are left to the transport.
    add_outcome_unknown: &["Request timed out", "System is abnormal"],
    // Exchanges::Bitget#minimum_amount_logic: market :base_or_quote, limit :base_and_quote_in_base (#532).
    minimum_logic: MinimumLogic { market: AmountLogic::BaseOrQuote, limit: AmountLogic::BaseAndQuoteInBase },
    // Honeymaker::Clients::Bitget#place_order sends BigDecimal#to_s('F') strings, one `size` in the order's unit.
    wire: WireFormat::Bitget,
    deadline_sent: false,
    reach_within_secs: 55,
    absence_margin_secs: 1200,
    transport_raises: false,
    // Bot::FetchAndUpdateOrderJob asks Exchanges::Bitget#get_order: one order, strict.
    follow_up_strict: true, market_hours: false,
    // #get_orders names a gone order (40109, 43001, an empty list) under `missing` (G5).
    reports_missing: true,
    // Its client-order lookup can only FIND an order (plan VB §1.5).
    absence_provable: false,
    young_missing_fails: true,
    price_rule: PriceRule::Decimals,
    cross: None,
};

/// Exchange#transient_error? for MEXC: Client::NETWORK_TRANSIENT_PATTERNS plus Exchanges::Mexc::ERRORS[:transient].
const MEXC_TRANSIENT: [&str; 16] = ["Net::ReadTimeout", "Net::OpenTimeout", "Faraday::TimeoutError", "Faraday::ConnectionFailed",
    "execution expired", "Connection reset", "Errno::ECONNRESET", "connection refused", "Connection refused", "Errno::ECONNREFUSED",
    "end of file reached", "unexpected eof while reading",
    "Timestamp for this request is outside of the recvWindow", "Timestamp for this request was", "service not available", "Gateway Time-out"];

pub static MEXC: VenueRules = VenueRules {
    exchange_type: "Exchanges::Mexc",
    name: "MEXC",
    // Exchanges::Mexc::ERRORS (mexc.rb:3-20, AUDIT-mexc X4), matched on #parse_error_message's `msg`.
    kinds: [("insufficient_funds", &["Insufficient balance", "Insufficient position", "Oversold"]),
            ("invalid_key", &["Api key info invalid", "API-key format invalid", "Invalid Api-Key ID.", "Signature for this request is not valid",
                              "Signature verification failed"]),
            ("permission_denied", &["No permission to access the endpoint", "IP non white list"]), ("restricted", &[]),
            ("throttle", &["Too Many Requests", "Too many requests", "Requests are too frequent"]),
            ("transient", &["Timestamp for this request is outside of the recvWindow", "Timestamp for this request was", "service not available", "Gateway Time-out"])],
    transient_text: &MEXC_TRANSIENT,
    // Exchange#ambiguous_placement_error?: ERRORS[:transient] leaves a placement unknown whatever the HTTP status, but the
    // -1021 strings, placement-safe (placement::PLACEMENT_SAFE_TRANSIENT_ERRORS).
    add_outcome_unknown: &["service not available", "Gateway Time-out"],
    // Exchanges::Mexc#minimum_amount_logic: market :base_and_quote (Rails runs the :base_or_quote branch), limit
    // :base_and_quote_in_base. A zero base minimum (PEPEUSDT's baseSizePrecision) leaves the quote minimum the only floor.
    minimum_logic: MinimumLogic { market: AmountLogic::BaseOrQuote, limit: AmountLogic::BaseAndQuoteInBase },
    // Honeymaker::Clients::Mexc#new_order sends BigDecimal#to_s('F') strings.
    wire: WireFormat::Kraken,
    deadline_sent: false,
    reach_within_secs: 55,
    absence_margin_secs: 1200,
    transport_raises: false,
    // Bot::FetchAndUpdateOrderJob asks Exchanges::Mexc#get_order: one order, strict.
    follow_up_strict: true, market_hours: false,
    // #get_orders names a gone order under `missing` (X5).
    reports_missing: true,
    // Plan VB §1.5: the client-id lookup can only FIND an order; an unfound one waits for `deltabadger resolve-placement`.
    absence_provable: false,
    // #authoritative_missing_orders? is Exchange's false: a young order left out fails the sweep.
    young_missing_fails: true,
    price_rule: PriceRule::Decimals,
    cross: None,
};

/// The rules of a venue the engine trades on (`venue::registry`).
pub fn for_exchange(exchange_type: &str) -> Option<&'static VenueRules> { crate::venue::registry::trading_rules(exchange_type) }

/// Exchange#transient_error? for Gemini: Client::NETWORK_TRANSIENT_PATTERNS plus Exchanges::Gemini::ERRORS[:transient].
const GEMINI_TRANSIENT: [&str; 15] = ["Net::ReadTimeout", "Net::OpenTimeout", "Faraday::TimeoutError", "Faraday::ConnectionFailed",
    "execution expired", "Connection reset", "Errno::ECONNRESET", "connection refused", "Connection refused", "Errno::ECONNREFUSED",
    "end of file reached", "unexpected eof while reading",
    "InvalidNonce", "\"reason\":\"System\"", "\"reason\":\"Maintenance\""];

pub static GEMINI: VenueRules = VenueRules {
    exchange_type: "Exchanges::Gemini",
    name: "Gemini",
    // Exchanges::Gemini::ERRORS (gemini.rb:16-37), matched inside the error body; pinned by tests/gemini.rs.
    kinds: [("insufficient_funds", &["InsufficientFunds", "Insufficient Funds"]), ("invalid_key", &["InvalidSignature", "InvalidApiKey"]),
            ("permission_denied", &["MissingRole"]), ("restricted", &[]), ("throttle", &["RateLimit"]),
            ("transient", &["InvalidNonce", "\"reason\":\"System\"", "\"reason\":\"Maintenance\""])],
    transient_text: &GEMINI_TRANSIENT,
    // Exchange#ambiguous_placement_error?: transient_error? leaves a placement unknown, except the placement-safe InvalidNonce
    // (placement::PLACEMENT_SAFE_TRANSIENT_ERRORS) it checks first. The network patterns are left to the transport.
    add_outcome_unknown: &["\"reason\":\"System\"", "\"reason\":\"Maintenance\""],
    // Exchanges::Gemini#minimum_amount_logic: :base for every order.
    minimum_logic: MinimumLogic { market: AmountLogic::Base, limit: AmountLogic::Base },
    // Honeymaker::Clients::Gemini#new_order sends BigDecimal#to_s('F') strings.
    wire: WireFormat::Kraken,
    deadline_sent: false,
    reach_within_secs: 55,
    absence_margin_secs: 1200,
    transport_raises: false,
    // Bot::FetchAndUpdateOrderJob asks Exchanges::Gemini#get_order: one order, strict.
    follow_up_strict: true, market_hours: false,
    // #get_orders names an OrderNotFound order under `missing` (M3).
    reports_missing: true,
    // Its client-order lookup can only FIND an order (plan VB §1.5).
    absence_provable: false,
    young_missing_fails: true,
    price_rule: PriceRule::Decimals,
    // Exchanges::Gemini::MARKET_CROSS: a market order is an IOC limit across the book.
    cross: Some(MARKET_CROSS),
};

pub static HYPERLIQUID: VenueRules = VenueRules {
    exchange_type: "Exchanges::Hyperliquid",
    name: "Hyperliquid",
    // Exchanges::Hyperliquid::ERRORS (post-audit H1), matched inside "Hyperliquid order failed: …"; pinned by tests/hyperliquid.rs.
    kinds: [("insufficient_funds", &["Insufficient spot balance", "Insufficient balance", "Not enough balance"]), ("invalid_key", &["User or API Wallet"]),
            ("permission_denied", &[]), ("restricted", &[]), ("throttle", &["Too many cumulative requests sent", "HTTP 429", "status 429"]),
            ("transient", &[])],
    transient_text: &NETWORK_TRANSIENT_PATTERNS,
    // A placement's outcome is decided by HTTP status and transport in HyperliquidVenue::add_order, never by text.
    add_outcome_unknown: &[],
    // Exchanges::Hyperliquid#minimum_amount_logic: :base_and_quote_in_base (the 10-USDC floor as a base minimum).
    minimum_logic: MinimumLogic { market: AmountLogic::BaseAndQuoteInBase, limit: AmountLogic::BaseAndQuoteInBase },
    // The adapter parses the volume and price back and writes them through hyperliquid-rb's float_to_wire.
    wire: WireFormat::Kraken,
    deadline_sent: false,
    reach_within_secs: 55,
    absence_margin_secs: 1200,
    transport_raises: false,
    // Bot::FetchAndUpdateOrderJob asks #get_order, which recovers an unknownOid from the fills and else names it not found
    // for Bot::StaleOrderResolver: the lenient poll of that one order, fills recovery included (as Bitvavo's).
    follow_up_strict: false, market_hours: false,
    reports_missing: true,
    // Plan VB §1.5: the cloid lookup can only FIND an order.
    absence_provable: false,
    // #authoritative_missing_orders? true: the fills walk exhausts the history, so a young order left out just waits.
    young_missing_fails: false,
    price_rule: HYPERLIQUID_PRICE_RULE,
    // Exchanges::Hyperliquid::MARKET_CROSS: a market order is a GTC limit across the book.
    cross: Some(MARKET_CROSS),
};

/// Exchanges::Bybit rules; D-BYBIT-AMBIG retains uncertain placements.
const BYBIT_TRANSIENT: [&str; 13] = ["Net::ReadTimeout", "Net::OpenTimeout", "Faraday::TimeoutError", "Faraday::ConnectionFailed",
    "execution expired", "Connection reset", "Errno::ECONNRESET", "connection refused", "Connection refused", "Errno::ECONNREFUSED",
    "end of file reached", "unexpected eof while reading",
    "Timestamp for this request is outside of the recvWindow"];

pub static BYBIT: VenueRules = VenueRules {
    exchange_type: "Exchanges::Bybit",
    name: "Bybit",
    kinds: [("insufficient_funds", &["Insufficient balance"]),
            ("invalid_key", &["Invalid API-key, IP, or permissions for action.", "API key is invalid"]),
            ("permission_denied", &["Permission denied, please check your API key permissions"]),
            ("restricted", &[]), ("throttle", &[]), ("transient", &["Timestamp for this request is outside of the recvWindow"])],
    transient_text: &BYBIT_TRANSIENT,
    add_outcome_unknown: &["Server Timeout", "Server error."],
    minimum_logic: MinimumLogic { market: AmountLogic::BaseOrQuote, limit: AmountLogic::BaseAndQuoteInBase },
    wire: WireFormat::Bybit,
    deadline_sent: false,
    reach_within_secs: 55,
    absence_margin_secs: 1200,
    transport_raises: false,
    follow_up_strict: true, market_hours: false,
    reports_missing: true,
    absence_provable: false,
    young_missing_fails: true,
    price_rule: PriceRule::Decimals,
    cross: None,
};

/// Exchanges::Bitrue::ERRORS and minimum_amount_logic (post-audit T5/T8).
const BITRUE_TRANSIENT: [&str; 18] = ["Net::ReadTimeout", "Net::OpenTimeout", "Faraday::TimeoutError", "Faraday::ConnectionFailed",
    "execution expired", "Connection reset", "Errno::ECONNRESET", "connection refused", "Connection refused", "Errno::ECONNREFUSED",
    "end of file reached", "unexpected eof while reading",
    "recvWindow IllegalArgumentException", "timestamp IllegalArgumentException", "System abnormality", "Service unavailable", "System is under maintenance", "An unknown error occured"];

pub static BITRUE: VenueRules = VenueRules {
    exchange_type: "Exchanges::Bitrue",
    name: "Bitrue",
    kinds: [("insufficient_funds", &["Insufficient balance", "Insufficient account balance", "Account has insufficient balance"]),
            ("invalid_key", &["No login", "API key does not exist", "Invalid Api-Key ID.", "Signature for this request is not valid."]),
            ("permission_denied", &["You don't have permission", "has no operation privilege"]), ("restricted", &[]),
            ("throttle", &["Too Many Requests", "Too many requests", "Too much request weight", "Too many new orders"]),
            ("transient", &["recvWindow IllegalArgumentException", "timestamp IllegalArgumentException", "System abnormality", "Service unavailable", "System is under maintenance", "An unknown error occured"])],
    transient_text: &BITRUE_TRANSIENT,
    add_outcome_unknown: &["System abnormality", "Service unavailable", "System is under maintenance", "An unknown error occured"],
    minimum_logic: MinimumLogic { market: AmountLogic::BaseAndQuoteInBase, limit: AmountLogic::BaseAndQuoteInBase },
    wire: WireFormat::Kraken,
    deadline_sent: false,
    reach_within_secs: 55,
    absence_margin_secs: 1200,
    transport_raises: false,
    follow_up_strict: true, market_hours: false,
    reports_missing: true,
    absence_provable: false,
    young_missing_fails: true,
    price_rule: PriceRule::Decimals,
    cross: None,
};

/// Exchanges::Coinbase: market size in either unit; limits in base; batch missing is non-authoritative.
pub static COINBASE: VenueRules = VenueRules {
    exchange_type: "Exchanges::Coinbase", name: "Coinbase",
    kinds: [("insufficient_funds", &["Insufficient balance in source account"]), ("invalid_key", &[]),
            ("permission_denied", &[]), ("restricted", &[]), ("throttle", &[]), ("transient", &[])],
    transient_text: &NETWORK_TRANSIENT_PATTERNS, add_outcome_unknown: &[],
    minimum_logic: MinimumLogic { market: AmountLogic::BaseOrQuote, limit: AmountLogic::BaseAndQuoteInBase },
    wire: WireFormat::Kraken, deadline_sent: false, reach_within_secs: 55, absence_margin_secs: 1200,
    transport_raises: false, follow_up_strict: true, market_hours: false,
    reports_missing: true, absence_provable: false, young_missing_fails: true, price_rule: PriceRule::Decimals, cross: None,
};

/// Exchanges::Bingx::ERRORS, minimum_amount_logic and missing-order policy.
const BINGX_TRANSIENT: [&str; 15] = ["Net::ReadTimeout", "Net::OpenTimeout", "Faraday::TimeoutError", "Faraday::ConnectionFailed",
    "execution expired", "Connection reset", "Errno::ECONNRESET", "connection refused", "Connection refused", "Errno::ECONNREFUSED",
    "end of file reached", "unexpected eof while reading", "The current system is busy", "Internal system error", "timestamp mismatch"];
pub static BINGX: VenueRules = VenueRules {
    exchange_type: "Exchanges::Bingx", name: "BingX",
    kinds: [("insufficient_funds", &["Insufficient assets", "Insufficient balance", "insufficient balance"]),
        ("invalid_key", &["Incorrect apiKey", "Invalid Api-Key ID", "Signature verification failed", "Null signature"]),
        ("permission_denied", &["API key was created without the permission", "IP does not match IP whitelist"]),
        ("restricted", &[]), ("throttle", &["Too Many Requests", "rate limitation"]),
        ("transient", &["The current system is busy", "Internal system error", "timestamp mismatch"])],
    transient_text: &BINGX_TRANSIENT,
    add_outcome_unknown: &["The current system is busy", "Internal system error", "timestamp mismatch"],
    minimum_logic: MinimumLogic { market: AmountLogic::BaseOrQuote, limit: AmountLogic::BaseAndQuoteInBase },
    wire: WireFormat::Kraken, deadline_sent: false, reach_within_secs: 55, absence_margin_secs: 1200,
    transport_raises: false, follow_up_strict: true, market_hours: false, reports_missing: true,
    absence_provable: false, young_missing_fails: true, price_rule: PriceRule::Decimals, cross: None,
};
