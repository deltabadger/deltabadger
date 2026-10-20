//! One static row per venue this build knows (plan VB §1.1): the metadata Rails' Exchange subclasses answer
//! (`name`, `name_id`, `requires_passphrase?`, the rules table) and, field by field, what Rust has ported for it. An
//! `Option` that is `None` is not ported: the venue's keys get no job, no check and no wizard step. A row implies
//! nothing beyond its fields. Every list that used to name the venues (the sync lists, the wizard's, the engine's
//! admission, the key check's) is read from here; the per-venue code stays where it is, picked by the typed fields.
use crate::engine::venue_rules::{self, VenueRules};

/// How a key's balances are synced (`sync::balances`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BalanceSync { Alpaca, Binance, Kucoin }

/// How a key's ledger is synced (`sync::ledger`, `sync::binance_ledger`, `sync::kucoin_ledger`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerSync {
    Alpaca, Binance, Kucoin,
    /// Binance.US: binance_ledger's run reads nothing and completes (Rails reads api.binance.com, which refuses the key),
    /// so the tracker never judges its history (`reads_history`).
    BinanceUs,
}

impl LedgerSync {
    /// Whether the run really reads a history: the venues the tracker's history-gap step judges (tracker::gaps).
    pub fn reads_history(self) -> bool { self != Self::BinanceUs }
}

/// How a key is checked (ApiKey#get_validity): Alpaca's account read (`settings::validator::check`), or the venue's own
/// request sequence (`settings::key_check`) through its proxy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCheck { Alpaca, Kraken, Binance, Kucoin }

/// Which market data the figures read for the venue's bots (figures::page_market).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Figures {
    /// Prices and candles.
    Alpaca,
    /// allTickers prices and market candles.
    Kucoin,
}

impl Figures {
    pub fn candles(self) -> bool { matches!(self, Self::Alpaca | Self::Kucoin) }
}

pub struct Row {
    /// exchanges.type.
    pub exchange_type: &'static str,
    /// The venue as messages name it (VenueRules::name where it trades; the label of its transport's stderr lines).
    pub name: &'static str,
    /// Exchange#name_id, also ExchangeProxy's name (`proxy_<name_id>`, `PROXY_<NAME_ID>`).
    pub name_id: &'static str,
    /// The API host its clients send to (Alpaca's host is per key: `alpaca::Urls`).
    pub base: &'static str,
    /// The engine's rules where it trades.
    pub rules: Option<&'static VenueRules>,
    /// Exchange#requires_passphrase?.
    pub passphrase: bool,
    /// Its trading adapter reads the exchange's tickers (`binance::Pairs`) when the factory builds it.
    pub needs_pairs: bool,
    pub trades: bool,
    pub balances: Option<BalanceSync>,
    pub ledger: Option<LedgerSync>,
    pub key_check: Option<KeyCheck>,
    /// The wizard and settings take its keys (every other venue's key route is a named refusal).
    pub wizard: bool,
    pub figures: Option<Figures>,
}

pub static ROWS: [Row; 5] = [
    Row { exchange_type: crate::sync::ALPACA, name: "Alpaca", name_id: "alpaca", base: "", rules: Some(&venue_rules::ALPACA), passphrase: true,
          needs_pairs: false, trades: true, balances: Some(BalanceSync::Alpaca), ledger: Some(LedgerSync::Alpaca), key_check: Some(KeyCheck::Alpaca),
          wizard: true, figures: Some(Figures::Alpaca) },
    Row { exchange_type: "Exchanges::Kraken", name: "Kraken", name_id: "kraken", base: super::kraken::URL, rules: Some(&venue_rules::KRAKEN), passphrase: false,
          needs_pairs: false, trades: true, balances: None, ledger: None, key_check: Some(KeyCheck::Kraken), wizard: true, figures: None },
    Row { exchange_type: super::binance::BINANCE, name: "Binance", name_id: "binance", base: super::binance::URL, rules: Some(&venue_rules::BINANCE),
          passphrase: false, needs_pairs: true, trades: true, balances: Some(BalanceSync::Binance), ledger: Some(LedgerSync::Binance),
          key_check: Some(KeyCheck::Binance), wizard: true, figures: None },
    // Tracker reads only: its balances through BinanceVenue at US_URL; its key step a named refusal (settings::key_check).
    Row { exchange_type: super::binance::BINANCE_US, name: "Binance.US", name_id: "binance_us", base: super::binance::US_URL, rules: None,
          passphrase: false, needs_pairs: true, trades: false, balances: Some(BalanceSync::Binance), ledger: Some(LedgerSync::BinanceUs),
          key_check: None, wizard: false, figures: None },
    Row { exchange_type: super::kucoin::KUCOIN, name: "KuCoin", name_id: "kucoin", base: super::kucoin::URL, rules: Some(&venue_rules::KUCOIN),
          passphrase: true, needs_pairs: true, trades: true, balances: Some(BalanceSync::Kucoin), ledger: Some(LedgerSync::Kucoin),
          key_check: Some(KeyCheck::Kucoin), wizard: true, figures: Some(Figures::Kucoin) },
];

pub fn row(exchange_type: &str) -> Option<&'static Row> { ROWS.iter().find(|r| r.exchange_type == exchange_type) }

/// The exchange types whose rows `pick` holds, in table order.
pub fn types(pick: impl Fn(&Row) -> bool) -> Vec<&'static str> { ROWS.iter().filter(|r| pick(r)).map(|r| r.exchange_type).collect() }

/// The rules of a venue the engine trades on (`venue_rules::for_exchange`).
pub fn trading_rules(exchange_type: &str) -> Option<&'static VenueRules> { row(exchange_type).filter(|r| r.trades).and_then(|r| r.rules) }

/// ExchangeProxy's name for a venue whose key check is the venue's own (not Alpaca's account read).
pub fn key_check_proxy(exchange_type: &str) -> Option<&'static str> {
    row(exchange_type).filter(|r| r.key_check.is_some_and(|k| k != KeyCheck::Alpaca)).map(|r| r.name_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_agrees_with_its_rules_and_its_name_id() {
        for r in &ROWS {
            assert_eq!(r.name_id, crate::tracker::rows::name_id(r.exchange_type), "{}", r.exchange_type);
            if let Some(rules) = r.rules { assert_eq!((rules.exchange_type, rules.name), (r.exchange_type, r.name)); }
            assert!(!r.trades || r.rules.is_some(), "{}: a venue trades by its rules", r.exchange_type);
        }
        assert_eq!(ROWS.iter().map(|r| r.exchange_type).collect::<std::collections::HashSet<_>>().len(), ROWS.len());
    }

    /// What each list the registry replaced named before it (plan VB §1.1): the derivations name the same venues.
    #[test]
    fn the_derived_lists_are_the_lists_they_replaced() {
        let (alpaca, kraken, binance, us, kucoin) = ("Exchanges::Alpaca", "Exchanges::Kraken", "Exchanges::Binance", "Exchanges::BinanceUs", "Exchanges::Kucoin");
        assert_eq!(crate::sync::balance_venues(), [alpaca, binance, us, kucoin], "BALANCE_VENUES");
        assert_eq!(crate::sync::ledger_venues(), [alpaca, binance, us, kucoin], "LEDGER_VENUES");
        assert_eq!(crate::sync::ledger_read_venues(), [alpaca, binance, kucoin], "LEDGER_READ_VENUES");
        assert_eq!(types(|r| r.wizard), [alpaca, kraken, binance, kucoin], "wizard::keys::PORTED");
        assert_eq!(types(|r| r.trades), [alpaca, kraken, binance, kucoin], "venue_rules::for_exchange, eligibility::admission, LiveFactory");
        assert_eq!(types(|r| r.passphrase), [alpaca, kucoin], "model::credentials_with_redaction's passphrase");
        assert_eq!(types(|r| r.figures.is_some()), [alpaca, kucoin], "page_market::reads");
        assert_eq!(types(|r| r.figures.is_some_and(Figures::candles)), [alpaca, kucoin], "page_market::reads_candles");
        assert_eq!(types(|r| r.needs_pairs && r.trades), [binance, kucoin], "LiveFactory's Pairs::load");
        let proxies: Vec<_> = ROWS.iter().filter_map(|r| key_check_proxy(r.exchange_type).map(|p| (r.exchange_type, p, r.base))).collect();
        assert_eq!(proxies, [(kraken, "kraken", crate::venue::kraken::URL), (binance, "binance", crate::venue::binance::URL), (kucoin, "kucoin", crate::venue::kucoin::URL)],
                   "validator::key_check_venue, venue_key_urls, Connect::key_check_egress");
        assert_eq!(crate::venue::binance::host(binance), Some((crate::venue::binance::URL, "binance")));
        assert_eq!(crate::venue::binance::host(us), Some((crate::venue::binance::US_URL, "binance_us")));
        assert_eq!(crate::venue::binance::host(kucoin), None);
        for (ty, tag) in [(alpaca, "Alpaca"), (binance, "Binance"), (kucoin, "KuCoin")] { assert_eq!(row(ty).unwrap().name, tag); }
    }
}
