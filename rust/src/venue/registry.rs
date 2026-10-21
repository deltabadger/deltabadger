//! One static row per venue this build knows (plan VB §1.1): the metadata Rails' Exchange subclasses answer
//! (`name`, `name_id`, `requires_passphrase?`, the rules table) and, field by field, what Rust has ported for it. An
//! `Option` that is `None` is not ported: the venue's keys get no job, no check and no wizard step. A row implies
//! nothing beyond its fields. Every list that used to name the venues (the sync lists, the wizard's, the engine's
//! admission, the key check's) is read from here; the per-venue code stays where it is, picked by the typed fields.
use crate::engine::venue_rules::{self, VenueRules};

/// How a key's balances are synced (`sync::balances`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BalanceSync { Alpaca, Binance, Kucoin, Bitvavo, Gemini, Bitget, Mexc, Bitrue }

/// How a key's ledger is synced (`sync::ledger`, `sync::binance_ledger` (Binance and Binance.US, each its own sources),
/// `sync::kucoin_ledger`, `sync::bitvavo_ledger`, `sync::gemini_ledger`, `sync::bitget_ledger`, `sync::mexc_ledger`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerSync { Alpaca, Binance, Kucoin, Bitvavo, Gemini, Bitget, Mexc, Bitrue }

/// How a key is checked (ApiKey#get_validity): Alpaca's account read (`settings::validator::check`), or the venue's own
/// request sequence (`settings::key_check`) through its proxy. Binance.US's is its own (binance_us.rb: an account read,
/// then a cancel probe), on Binance's wire. Bitvavo's reads the balance, then probes a cancel, for a reading key too. Bitget's
/// probes a cancel (a withdrawal key reads its assets; a reading key its balances). MEXC's reads the account's canTrade,
/// and for a reading key the deposit history too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCheck { Alpaca, Kraken, Binance, BinanceUs, Kucoin, Bitvavo, Gemini, Bitget, Mexc, Bitrue }

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

pub static ROWS: [Row; 10] = [
    Row { exchange_type: crate::sync::ALPACA, name: "Alpaca", name_id: "alpaca", base: "", rules: Some(&venue_rules::ALPACA), passphrase: true,
          needs_pairs: false, trades: true, balances: Some(BalanceSync::Alpaca), ledger: Some(LedgerSync::Alpaca), key_check: Some(KeyCheck::Alpaca),
          wizard: true, figures: Some(Figures::Alpaca) },
    Row { exchange_type: "Exchanges::Kraken", name: "Kraken", name_id: "kraken", base: super::kraken::URL, rules: Some(&venue_rules::KRAKEN), passphrase: false,
          needs_pairs: false, trades: true, balances: None, ledger: None, key_check: Some(KeyCheck::Kraken), wizard: true, figures: None },
    Row { exchange_type: super::binance::BINANCE, name: "Binance", name_id: "binance", base: super::binance::URL, rules: Some(&venue_rules::BINANCE),
          passphrase: false, needs_pairs: true, trades: true, balances: Some(BalanceSync::Binance), ledger: Some(LedgerSync::Binance),
          key_check: Some(KeyCheck::Binance), wizard: true, figures: None },
    // Binance configured (Exchanges::BinanceUs < Exchanges::Binance): BinanceVenue at US_URL with its own rules row, its own
    // key check, and its ledger's own source list (binance_ledger).
    Row { exchange_type: super::binance::BINANCE_US, name: "Binance.US", name_id: "binance_us", base: super::binance::US_URL, rules: Some(&venue_rules::BINANCE_US),
          passphrase: false, needs_pairs: true, trades: true, balances: Some(BalanceSync::Binance), ledger: Some(LedgerSync::Binance),
          key_check: Some(KeyCheck::BinanceUs), wizard: true, figures: None },
    Row { exchange_type: super::kucoin::KUCOIN, name: "KuCoin", name_id: "kucoin", base: super::kucoin::URL, rules: Some(&venue_rules::KUCOIN),
          passphrase: true, needs_pairs: true, trades: true, balances: Some(BalanceSync::Kucoin), ledger: Some(LedgerSync::Kucoin),
          key_check: Some(KeyCheck::Kucoin), wizard: true, figures: Some(Figures::Kucoin) },
    // Plan VB slice A3. Its figures land with family F2 (AF): until then its bots show no exchange note.
    Row { exchange_type: super::bitvavo::BITVAVO, name: "Bitvavo", name_id: "bitvavo", base: super::bitvavo::URL, rules: Some(&venue_rules::BITVAVO),
          passphrase: false, needs_pairs: true, trades: true, balances: Some(BalanceSync::Bitvavo), ledger: Some(LedgerSync::Bitvavo),
          key_check: Some(KeyCheck::Bitvavo), wizard: true, figures: None },
    // Its figures land with the F3 family (plan VB §1.7, BF): until then its bots show no exchange note.
    Row { exchange_type: super::gemini::GEMINI, name: "Gemini", name_id: "gemini", base: super::gemini::URL, rules: Some(&venue_rules::GEMINI),
          passphrase: false, needs_pairs: true, trades: true, balances: Some(BalanceSync::Gemini), ledger: Some(LedgerSync::Gemini),
          key_check: Some(KeyCheck::Gemini), wizard: true, figures: None },
    // Plan VB slice A4, the classic v2 API as Rails runs it. Its figures land with family F2 (AF): until then its bots show
    // no exchange note.
    Row { exchange_type: super::bitget::BITGET, name: "Bitget", name_id: "bitget", base: super::bitget::URL, rules: Some(&venue_rules::BITGET),
          passphrase: true, needs_pairs: true, trades: true, balances: Some(BalanceSync::Bitget), ledger: Some(LedgerSync::Bitget),
          key_check: Some(KeyCheck::Bitget), wizard: true, figures: None },
    // Plan VB slice A5. Its figures land with family F1 (AF): until then its bots show no exchange note.
    Row { exchange_type: super::mexc::MEXC, name: "MEXC", name_id: "mexc", base: super::mexc::URL, rules: Some(&venue_rules::MEXC),
          passphrase: false, needs_pairs: true, trades: true, balances: Some(BalanceSync::Mexc), ledger: Some(LedgerSync::Mexc),
          key_check: Some(KeyCheck::Mexc), wizard: true, figures: None },
    Row { exchange_type: super::bitrue::BITRUE, name: "Bitrue", name_id: "bitrue", base: super::bitrue::URL, rules: Some(&venue_rules::BITRUE),
          passphrase: false, needs_pairs: true, trades: true, balances: Some(BalanceSync::Bitrue), ledger: Some(LedgerSync::Bitrue),
          key_check: Some(KeyCheck::Bitrue), wizard: true, figures: None },
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
        let (alpaca, kraken, binance, us, kucoin, bitvavo, gemini, bitget, mexc, bitrue) = ("Exchanges::Alpaca", "Exchanges::Kraken", "Exchanges::Binance", "Exchanges::BinanceUs",
                                                                                 "Exchanges::Kucoin", "Exchanges::Bitvavo", "Exchanges::Gemini", "Exchanges::Bitget", "Exchanges::Mexc", "Exchanges::Bitrue");
        assert_eq!(crate::sync::balance_venues(), [alpaca, binance, us, kucoin, bitvavo, gemini, bitget, mexc, bitrue], "BALANCE_VENUES");
        assert_eq!(crate::sync::ledger_venues(), [alpaca, binance, us, kucoin, bitvavo, gemini, bitget, mexc, bitrue], "LEDGER_VENUES");
        assert_eq!(types(|r| r.wizard), [alpaca, kraken, binance, us, kucoin, bitvavo, gemini, bitget, mexc, bitrue], "wizard::keys::PORTED");
        assert_eq!(types(|r| r.trades), [alpaca, kraken, binance, us, kucoin, bitvavo, gemini, bitget, mexc, bitrue], "venue_rules::for_exchange, eligibility::admission, LiveFactory");
        assert_eq!(types(|r| r.passphrase), [alpaca, kucoin, bitget], "model::credentials_with_redaction's passphrase");
        assert_eq!(types(|r| r.figures.is_some()), [alpaca, kucoin], "page_market::reads");
        assert_eq!(types(|r| r.figures.is_some_and(Figures::candles)), [alpaca, kucoin], "page_market::reads_candles");
        assert_eq!(types(|r| r.needs_pairs && r.trades), [binance, us, kucoin, bitvavo, gemini, bitget, mexc, bitrue], "LiveFactory's Pairs::load");
        let proxies: Vec<_> = ROWS.iter().filter_map(|r| key_check_proxy(r.exchange_type).map(|p| (r.exchange_type, p, r.base))).collect();
        assert_eq!(proxies, [(kraken, "kraken", crate::venue::kraken::URL), (binance, "binance", crate::venue::binance::URL), (us, "binance_us", crate::venue::binance::US_URL),
                             (kucoin, "kucoin", crate::venue::kucoin::URL), (bitvavo, "bitvavo", crate::venue::bitvavo::URL), (gemini, "gemini", crate::venue::gemini::URL),
                             (bitget, "bitget", crate::venue::bitget::URL), (mexc, "mexc", crate::venue::mexc::URL), (bitrue, "bitrue", crate::venue::bitrue::URL)],
                   "validator::key_check_venue, venue_key_urls, Connect::key_check_egress");
        assert_eq!(crate::venue::binance::host(binance), Some((crate::venue::binance::URL, "binance")));
        assert_eq!(crate::venue::binance::host(us), Some((crate::venue::binance::US_URL, "binance_us")));
        assert_eq!(crate::venue::binance::host(kucoin), None);
        for (ty, tag) in [(alpaca, "Alpaca"), (binance, "Binance"), (us, "Binance.US"), (kucoin, "KuCoin"), (bitvavo, "Bitvavo"), (gemini, "Gemini"), (bitget, "Bitget"), (mexc, "MEXC"), (bitrue, "Bitrue")] { assert_eq!(row(ty).unwrap().name, tag); }
    }
}
