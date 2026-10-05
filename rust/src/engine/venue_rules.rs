//! What Rails' Exchange subclasses decide differently per venue, as one table per venue selected by exchange type:
//! Exchanges::*::ERRORS with Exchange#failure_kind (substring, first kind wins, FAILURE_KINDS order), the polls'
//! #transient_error? / #throttled_error?, #minimum_amount_logic, the order wire format, and when an order the engine
//! sent can be proven absent.

/// Exchange#minimum_amount_logic for a buy (Bot::OrderSetter#calculate_best_amount_info).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MinimumLogic {
    /// Exchanges::Kraken: a market buy is :base_or_quote, a limit buy :base.
    KrakenBaseOrQuote,
    /// Exchanges::Alpaca: always :quote, against minimum_quote_size only.
    Quote,
}

/// How an OrderPlan becomes the strings the venue receives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WireFormat {
    /// Honeymaker's Kraken client: BigDecimal#to_s('F'); a market buy may be `viqc` (volume in quote).
    Kraken,
    /// Exchanges::Alpaca#set_market_order / #set_limit_order: format('%.Nf', BigDecimal), which goes through Float;
    /// a market buy is `notional`, a limit buy `qty` + `limit_price`.
    Alpaca,
}

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
}

fn any(patterns: &[&str], messages: &[String]) -> bool { patterns.iter().any(|p| messages.iter().any(|m| m.contains(p))) }

impl VenueRules {
    pub fn failure_kind(&self, messages: &[String]) -> Option<&'static str> {
        self.kinds.iter().find(|(_, pats)| any(pats, messages)).map(|(k, _)| *k)
    }
    pub fn is_throttle(&self, messages: &[String]) -> bool { self.kinds.iter().any(|(k, pats)| *k == "throttle" && any(pats, messages)) }
    pub fn is_transient(&self, messages: &[String]) -> bool { any(self.transient_text, messages) }
    pub fn add_outcome_unknown(&self, messages: &[String]) -> bool { any(self.add_outcome_unknown, messages) }
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
    minimum_logic: MinimumLogic::KrakenBaseOrQuote,
    wire: WireFormat::Kraken,
    deadline_sent: true,
    reach_within_secs: 10,
    absence_margin_secs: 0, // Kraken enforces the deadline server-side: absence is deadline + 60 s (placement::recover_since)
    transport_raises: false,
    follow_up_strict: false, market_hours: false,
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
    minimum_logic: MinimumLogic::Quote,
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
};

pub fn for_exchange(exchange_type: &str) -> Option<&'static VenueRules> {
    [&KRAKEN, &ALPACA].into_iter().find(|r| r.exchange_type == exchange_type)
}
