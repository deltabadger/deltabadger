//! Delisted coins (plan r10 §1.1), PR0: which coin a ledger leg (base, quote or fee) holds, decided from this venue's
//! evidence only. Pure and unwired: nothing calls it until PR1, so `ALIASES_V2` and `CUTS` change no request, no stored
//! row and no recorded asset (`prices::ALIASES` stays what sync and the walk read).
//!
//! The steps, in order, for one leg:
//! - −1. a broker's recorded security is that security;
//! - 0. a stablecoin spelling is cash; a fiat spelling is cash unless this venue gives coin evidence for this leg (steps
//!      2–3b), and the catalogue never overrides cash;
//! - 1. a candidate coin under a scaled or leveraged spelling of its own symbol is refused (`Unknown(Units)`);
//! - 2. a dated alias names its coin, a cut refuses;
//! - 3. the base leg's recorded asset, when this venue's listings bind it to the spelling; 3b. else this venue's own
//!      listing of the spelling on this side (one coin);
//! - 4. the catalogue (`CATALOGUE`: V, or strict).
//!
//! Every coin answer is held against the observed bindings (`listing_evidence`): an answer for a date inside an interval
//! in which this venue bound the spelling to another asset is `Unknown(Conflict)`.
use super::{fiat, stable};
use chrono::{DateTime, NaiveDate, Utc};
use std::collections::{BTreeSet, HashMap};

/// The catalogue step (owner decision 2). V: exactly one non-security asset carries the symbol, and it is a
/// `Cryptocurrency`. Strict: no catalogue step at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Catalogue { V, Strict }
/// The plan's recommendation; the owner has not decided yet (owner decision 2).
pub const CATALOGUE: Catalogue = Catalogue::V;

/// A catalogue asset as the resolver reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetRef { pub id: i64, pub external_id: Option<String>, pub symbol: Option<String>, pub category: Option<String> }

impl AssetRef {
    fn security(&self) -> bool { self.category.as_deref().is_some_and(|c| super::prices::STOCK_CATEGORIES.contains(&c)) }
    fn crypto(&self) -> bool { self.category.as_deref() == Some("Cryptocurrency") }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Side { Base, Quote, Fee }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CashKind { Fiat, Stable }

/// Why a leg's coin is not known. `Units`, `Cut` and `Conflict` (the evidence names two different coins for this leg)
/// are hard refusals: nothing is inherited or linked into or out of them (§1.2). `Unclear`: no evidence, or no single
/// answer from it (two listed coins, a live/stale disagreement, a fee whose sides disagree).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Why { Units, Cut, Conflict, Unclear }

/// What a leg holds. A coin or a security by its external id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Holding { Cash(CashKind), Coin(String), Security(String), Unknown(Why) }

impl Holding {
    /// A refusal no link or inheritance may cross.
    pub fn hard(&self) -> bool { matches!(self, Holding::Unknown(Why::Units | Why::Cut | Why::Conflict)) }
    pub fn known(&self) -> bool { !matches!(self, Holding::Unknown(_)) }
}

/// The venue a leg was booked on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VenueRef { pub id: i64, pub name_id: String, pub broker: bool }

/// One leg to resolve.
#[derive(Clone, Debug)]
pub struct Leg<'a> {
    pub venue: &'a VenueRef,
    pub side: Side,
    pub spelling: &'a str,
    pub at: DateTime<Utc>,
    /// The row's recorded `base_asset_id` (base legs only).
    pub recorded: Option<&'a AssetRef>,
}

/// One observed binding interval (`listing_evidence`): the venue bound the spelling on that side to `asset_id` at every
/// import from `from` to `to`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interval { pub asset_id: i64, pub from: DateTime<Utc>, pub to: DateTime<Utc> }

/// What the resolver reads, indexed in memory (PR1 loads it; PR0 tests build it by hand).
#[derive(Default, Clone, Debug)]
pub struct Facts {
    /// (venue, side, spelling) → the assets its live tickers bind to it, in ticker order.
    pub live: HashMap<(i64, Side, String), Vec<AssetRef>>,
    /// (venue, spelling) → the assets its tombstoned tickers (`__stale_<id>_<spelling>`) bound as a base.
    pub stale: HashMap<(i64, String), Vec<AssetRef>>,
    /// symbol → every catalogue asset with it.
    pub catalogue: HashMap<String, Vec<AssetRef>>,
    /// (venue, side, spelling) → the observed binding intervals.
    pub evidence: HashMap<(i64, Side, String), Vec<Interval>>,
}

/// (symbol, coin, venue, last day it holds): main's `prices::ALIASES`, plus Binance's BTG (Bitcoin Gold, not the rank
/// guess).
type Day = (i32, u32, u32);
type Alias = (&'static str, &'static str, Option<&'static str>, Option<Day>);
const ALIASES_V2: [Alias; 9] = [
    ("LUNA", "terra-luna", None, Some((2022, 5, 27))),
    ("QUICK", "quick", None, Some((2023, 7, 20))),
    ("MATIC", "matic-network", None, None),
    ("LIT", "litentry", Some("binance"), None),
    ("GAL", "project-galaxy", Some("binance"), None),
    ("DAR", "mines-of-dalarnia", Some("binance"), None),
    ("PNT", "pnetwork", None, None),
    ("BURGER", "burger-swap", None, None),
    ("BTG", "bitcoin-gold", Some("binance"), None),
];
/// (symbol, last day it holds; None: every day): a spelling that names no coin we can tell (Q-PHB, Q-STPT).
const CUTS: [(&str, Option<Day>); 2] = [("PHB", Some((2021, 11, 4))), ("STPT", None)];

const LEVERAGE: [&str; 10] = ["2L", "2S", "3L", "3S", "5L", "5S", "UP", "DOWN", "BULL", "BEAR"];

fn ymd((y, m, d): Day) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, d).unwrap_or(NaiveDate::MIN) }
fn within(until: Option<Day>, at: DateTime<Utc>) -> bool { until.is_none_or(|u| at.date_naive() <= ymd(u)) }

/// Whether `spelling` is a scaled (`1000REKT`, `1MBABYDOGE`) or leveraged (`BTCUP`, `ETH3L`) form of `symbol`.
pub fn other_units(spelling: &str, symbol: &str) -> bool {
    if spelling == symbol || symbol.is_empty() { return false; }
    // A scale is 10, 100, 1000 … or 1K, 10K, 1M …
    let scaled = spelling.strip_suffix(symbol).is_some_and(|scale| {
        let n = scale.strip_suffix(['K', 'M']).unwrap_or(scale);
        n.starts_with('1') && n[1..].bytes().all(|b| b == b'0') && (n.len() > 1 || n.len() < scale.len())
    });
    let levered = spelling.strip_prefix(symbol).is_some_and(|rest| LEVERAGE.contains(&rest));
    scaled || levered
}

/// A coin answer for `leg`: refused under another unit's spelling, or when this venue's observed binding at the leg's
/// date names another asset.
fn coin(f: &Facts, leg: &Leg, asset: &AssetRef) -> Holding {
    if asset.symbol.as_deref().is_some_and(|s| other_units(leg.spelling, s)) { return Holding::Unknown(Why::Units); }
    let Some(id) = asset.external_id.clone() else { return Holding::Unknown(Why::Unclear) };
    if observed(f, leg).into_iter().any(|seen| seen != asset.id) { return Holding::Unknown(Why::Conflict); }
    Holding::Coin(id)
}

/// The asset ids this venue was observed binding the spelling to at the leg's date (on both sides for a fee leg).
fn observed(f: &Facts, leg: &Leg) -> BTreeSet<i64> {
    let sides: &[Side] = if leg.side == Side::Fee { &[Side::Base, Side::Quote] } else { std::slice::from_ref(&leg.side) };
    sides.iter().filter_map(|s| f.evidence.get(&(leg.venue.id, *s, leg.spelling.to_string()))).flatten()
        .filter(|i| i.from <= leg.at && leg.at <= i.to).map(|i| i.asset_id).collect()
}

/// An alias's answer, contradicted when this venue was observed binding the spelling to an asset whose external id is
/// not the alias's coin.
fn alias(f: &Facts, leg: &Leg, ext: &str) -> Holding {
    let assets: Vec<&AssetRef> = f.live.values().chain(f.stale.values()).chain(f.catalogue.values()).flatten().collect();
    let other = observed(f, leg).into_iter().any(|seen| assets.iter().find(|a| a.id == seen).is_none_or(|a| a.external_id.as_deref() != Some(ext)));
    if other { Holding::Unknown(Why::Conflict) } else { Holding::Coin(ext.to_string()) }
}

/// Step 2: a cut or a dated alias of this venue, in its window.
fn aliased(f: &Facts, leg: &Leg) -> Option<Holding> {
    if CUTS.iter().any(|(s, until)| *s == leg.spelling && within(*until, leg.at)) { return Some(Holding::Unknown(Why::Cut)); }
    ALIASES_V2.iter().find(|(s, _, v, until)| *s == leg.spelling && v.is_none_or(|v| v == leg.venue.name_id) && within(*until, leg.at))
        .map(|(_, ext, _, _)| alias(f, leg, ext))
}

/// Whether this venue's listings (live, stale, or observed) bind `asset` to the base spelling.
fn bound(f: &Facts, leg: &Leg, asset: &AssetRef) -> bool {
    let key = (leg.venue.id, leg.spelling.to_string());
    f.live.get(&(leg.venue.id, Side::Base, key.1.clone())).into_iter().chain(f.stale.get(&key)).flatten().any(|a| a.id == asset.id)
        || f.evidence.get(&(leg.venue.id, Side::Base, key.1)).into_iter().flatten().any(|i| i.asset_id == asset.id)
}

fn distinct(assets: impl IntoIterator<Item = AssetRef>) -> Vec<AssetRef> {
    let mut out: Vec<AssetRef> = vec![];
    for a in assets { if !out.iter().any(|o| o.id == a.id) { out.push(a); } }
    out
}

/// Step 3b: this venue's listing of the spelling on the leg's side(s), coins only: exact live, else stale (base only),
/// else Kraken's own-symbol match. One coin, else nothing; a live/stale disagreement is a conflict unless an observed
/// binding at the leg's date picks one of them.
fn listed(f: &Facts, leg: &Leg) -> Option<Holding> {
    let sides: &[Side] = if leg.side == Side::Fee { &[Side::Base, Side::Quote] } else { std::slice::from_ref(&leg.side) };
    let coins = |v: Option<&Vec<AssetRef>>| distinct(v.into_iter().flatten().filter(|a| a.crypto()).cloned());
    let live = distinct(sides.iter().flat_map(|s| coins(f.live.get(&(leg.venue.id, *s, leg.spelling.to_string())))));
    let stale = if sides.contains(&Side::Base) { coins(f.stale.get(&(leg.venue.id, leg.spelling.to_string()))) } else { vec![] };
    let own = || {
        let all = f.live.iter().filter(|((v, s, _), _)| *v == leg.venue.id && sides.contains(s)).flat_map(|(_, l)| l);
        distinct(all.filter(|a| a.crypto() && a.symbol.as_deref() == Some(leg.spelling)).cloned())
    };
    let candidates = if !live.is_empty() { live.clone() } else if !stale.is_empty() { stale.clone() } else if leg.venue.name_id == "kraken" { own() } else { vec![] };
    let one = match candidates.as_slice() { [] => return None, [one] => one.clone(), _ => return Some(Holding::Unknown(Why::Unclear)) };
    let disagree = !live.is_empty() && stale.iter().any(|s| s.id != one.id);
    if disagree {
        let seen = observed(f, leg);
        return Some(match (seen.len(), seen.iter().next()) {
            (1, Some(id)) => live.iter().chain(&stale).find(|a| a.id == *id).map_or(Holding::Unknown(Why::Conflict), |a| coin(f, leg, a)),
            _ => Holding::Unknown(Why::Unclear),
        });
    }
    Some(coin(f, leg, &one))
}

/// Step 4: the catalogue.
fn catalogued(f: &Facts, leg: &Leg) -> Option<Holding> {
    if CATALOGUE == Catalogue::Strict { return None; }
    let all = distinct(f.catalogue.get(leg.spelling).into_iter().flatten().cloned());
    let others: Vec<&AssetRef> = all.iter().filter(|a| !a.security()).collect();
    match others.as_slice() { [one] if one.crypto() => Some(coin(f, leg, one)), _ => None }
}

/// A broker's unrecorded leg: one candidate (its listing, else the catalogue) of one type, else unknown.
fn broker_unrecorded(f: &Facts, leg: &Leg) -> Holding {
    let listing = distinct(f.live.get(&(leg.venue.id, leg.side, leg.spelling.to_string())).into_iter().flatten().cloned());
    let candidates = if listing.is_empty() { distinct(f.catalogue.get(leg.spelling).into_iter().flatten().cloned()) } else { listing };
    match candidates.as_slice() {
        [one] if one.security() => one.external_id.clone().map_or(Holding::Unknown(Why::Unclear), Holding::Security),
        [one] if one.crypto() => coin(f, leg, one),
        _ => Holding::Unknown(Why::Unclear),
    }
}

/// Steps 2–4 for a leg that is not cash by spelling (or a fiat spelling looking for coin evidence: `catalogue` false).
fn evidence(f: &Facts, leg: &Leg, catalogue: bool) -> Option<Holding> {
    if let Some(h) = aliased(f, leg) { return Some(h); }
    if let Some(recorded) = leg.recorded.filter(|a| leg.side == Side::Base && (a.crypto() || a.category.is_none() || a.category.as_deref() == Some("Tokenized Stock"))) {
        if bound(f, leg, recorded) { return Some(coin(f, leg, recorded)); }
        // Not bound here: what the venue's listing (or the catalogue) says, a conflict when it names another coin.
        let rest = if leg.venue.broker { Some(broker_unrecorded(f, leg)) } else { listed(f, leg).or_else(|| if catalogue { catalogued(f, leg) } else { None }) };
        return match rest {
            Some(Holding::Coin(other)) if recorded.external_id.as_deref() != Some(other.as_str()) => Some(Holding::Unknown(Why::Conflict)),
            other => other,
        };
    }
    if leg.venue.broker { return Some(broker_unrecorded(f, leg)); }
    listed(f, leg).or_else(|| if catalogue { catalogued(f, leg) } else { None })
}

/// What `leg` holds (§1.1).
pub fn holding(f: &Facts, leg: &Leg) -> Holding {
    if let Some(recorded) = leg.recorded.filter(|a| leg.venue.broker && leg.side == Side::Base && a.security()) {
        if let Some(id) = &recorded.external_id { return Holding::Security(id.clone()); }
    }
    if stable(leg.spelling) { return Holding::Cash(CashKind::Stable); }
    if fiat(leg.spelling) {
        return match evidence(f, leg, false) { Some(h @ Holding::Coin(_)) => h, _ => Holding::Cash(CashKind::Fiat) };
    }
    evidence(f, leg, true).unwrap_or(Holding::Unknown(Why::Unclear))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(id: i64, ext: &str, symbol: &str, category: Option<&str>) -> AssetRef {
        AssetRef { id, external_id: Some(ext.into()), symbol: Some(symbol.into()), category: category.map(str::to_string) }
    }
    fn coin_asset(id: i64, ext: &str, symbol: &str) -> AssetRef { asset(id, ext, symbol, Some("Cryptocurrency")) }
    fn at(s: &str) -> DateTime<Utc> { format!("{s}T12:00:00Z").parse().unwrap() }
    fn venue(id: i64, name: &str, broker: bool) -> VenueRef { VenueRef { id, name_id: name.into(), broker } }
    fn leg<'a>(v: &'a VenueRef, side: Side, spelling: &'a str, day: &str, recorded: Option<&'a AssetRef>) -> Leg<'a> {
        Leg { venue: v, side, spelling, at: at(day), recorded }
    }
    fn coin_of(ext: &str) -> Holding { Holding::Coin(ext.into()) }
    fn list(f: &mut Facts, v: &VenueRef, side: Side, spelling: &str, a: &AssetRef) { f.live.entry((v.id, side, spelling.into())).or_default().push(a.clone()); }
    fn catalogue(f: &mut Facts, a: &AssetRef) { f.catalogue.entry(a.symbol.clone().unwrap()).or_default().push(a.clone()); }
    fn seen(f: &mut Facts, v: &VenueRef, side: Side, spelling: &str, asset_id: i64, from: &str, to: &str) {
        f.evidence.entry((v.id, side, spelling.into())).or_default().push(Interval { asset_id, from: at(from), to: at(to) });
    }

    /// Step 0: a stablecoin spelling is cash on every leg, though the catalogue files Tether as a coin; a fiat spelling
    /// is cash unless this venue lists a coin under it (MEXC base PLN is PLEARN), and a lone catalogue coin never makes
    /// it a coin (SEK).
    #[test]
    fn cash_first_by_spelling_and_fiat_yields_only_to_this_venues_coin_evidence() {
        let (binance, mexc) = (venue(1, "binance", false), venue(2, "mexc", false));
        let mut f = Facts::default();
        let tether = coin_asset(405, "tether", "USDT");
        catalogue(&mut f, &tether);
        list(&mut f, &binance, Side::Quote, "USDT", &tether);
        catalogue(&mut f, &coin_asset(900, "sek-coin", "SEK"));
        let plearn = coin_asset(2227, "pleasure-coin", "PLN");
        list(&mut f, &mexc, Side::Base, "PLN", &plearn);
        catalogue(&mut f, &plearn);
        assert_eq!(holding(&f, &leg(&binance, Side::Quote, "USDT", "2024-01-01", None)), Holding::Cash(CashKind::Stable));
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "USDT", "2024-01-01", Some(&tether))), Holding::Cash(CashKind::Stable));
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "SEK", "2024-01-01", None)), Holding::Cash(CashKind::Fiat), "the catalogue never overrides cash");
        assert_eq!(holding(&f, &leg(&mexc, Side::Base, "PLN", "2024-01-01", Some(&plearn))), coin_of("pleasure-coin"), "recorded and bound");
        assert_eq!(holding(&f, &leg(&mexc, Side::Base, "PLN", "2024-01-01", None)), coin_of("pleasure-coin"), "listed");
        assert_eq!(holding(&f, &leg(&mexc, Side::Quote, "PLN", "2024-01-01", None)), Holding::Cash(CashKind::Fiat), "MEXC lists no PLN quote coin");
        assert_eq!(holding(&f, &leg(&binance, Side::Quote, "PLN", "2024-01-01", None)), Holding::Cash(CashKind::Fiat), "a złoty quote elsewhere");
    }

    /// Step 1: a scaled or leveraged spelling of the candidate's own symbol is a hard refusal, on every step.
    #[test]
    fn another_units_spelling_is_refused() {
        let v = venue(1, "binance", false);
        let mut f = Facts::default();
        let rekt = coin_asset(657, "rekt", "REKT");
        list(&mut f, &v, Side::Base, "1000REKT", &rekt);
        catalogue(&mut f, &coin_asset(10, "bitcoin", "BTC"));
        assert_eq!(holding(&f, &leg(&v, Side::Base, "1000REKT", "2025-01-01", Some(&rekt))), Holding::Unknown(Why::Units));
        assert_eq!(holding(&f, &leg(&v, Side::Base, "1000REKT", "2025-01-01", None)), Holding::Unknown(Why::Units));
        assert!(holding(&f, &leg(&v, Side::Base, "1000REKT", "2025-01-01", None)).hard());
        for (spelling, symbol, other) in [("1MBABYDOGE", "BABYDOGE", true), ("10000SATS", "SATS", true), ("1000000MOG", "MOG", true), ("BTCUP", "BTC", true),
                                          ("ETH3L", "ETH", true), ("ETHBEAR", "ETH", true), ("REKT", "REKT", false), ("2000REKT", "REKT", false),
                                          ("1INCH", "INCH", false), ("BTCB", "BTC", false)] {
            assert_eq!(other_units(spelling, symbol), other, "{spelling} vs {symbol}");
        }
    }

    /// Step 2: Binance's BTG is Bitcoin Gold (main's rank guess is another coin); PHB before its relaunch and STPT are
    /// cuts; an alias only on its venue and in its window.
    #[test]
    fn aliases_and_cuts_in_their_windows() {
        let (binance, kucoin) = (venue(1, "binance", false), venue(2, "kucoin", false));
        let mut f = Facts::default();
        catalogue(&mut f, &coin_asset(31, "openverse-network", "BTG"));
        catalogue(&mut f, &asset(32, "bitcoin-gold", "BTG", None));
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "BTG", "2018-01-01", None)), coin_of("bitcoin-gold"));
        assert_eq!(holding(&f, &leg(&kucoin, Side::Base, "BTG", "2018-01-01", None)), Holding::Unknown(Why::Unclear), "V: a NULL-category twin makes two candidates");
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "PHB", "2021-11-04", None)), Holding::Unknown(Why::Cut));
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "STPT", "2025-01-01", None)), Holding::Unknown(Why::Cut));
        assert!(holding(&f, &leg(&binance, Side::Base, "STPT", "2025-01-01", None)).hard());
        catalogue(&mut f, &coin_asset(40, "phoenix-global", "PHB"));
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "PHB", "2021-11-05", None)), coin_of("phoenix-global"), "after the cut");
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "LUNA", "2022-05-27", None)), coin_of("terra-luna"));
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "LUNA", "2022-05-28", None)), Holding::Unknown(Why::Unclear), "past the window, no evidence");
        assert_eq!(holding(&f, &leg(&binance, Side::Fee, "BTG", "2018-01-01", None)), coin_of("bitcoin-gold"), "every leg");
    }

    /// Fable P2: a broker's recorded security wins before anything else, never overridden by an alias (IBKR LUNA, Alpaca
    /// USD.US); a broker's cash activity with nothing recorded is cash; an unrecorded broker leg needs one candidate.
    #[test]
    fn a_brokers_recorded_security_comes_first() {
        let (alpaca, ibkr) = (venue(5, "alpaca", true), venue(6, "ibkr", true));
        let mut f = Facts::default();
        let usd_us = asset(70, "usd-us-stock", "USD.US", Some("Stock"));
        let luna = asset(71, "luna-stock", "LUNA", Some("Common Stock"));
        assert_eq!(holding(&f, &leg(&alpaca, Side::Base, "USD", "2025-01-01", Some(&usd_us))), Holding::Security("usd-us-stock".into()));
        assert_eq!(holding(&f, &leg(&ibkr, Side::Base, "LUNA", "2022-01-01", Some(&luna))), Holding::Security("luna-stock".into()));
        assert_eq!(holding(&f, &leg(&alpaca, Side::Base, "USD", "2025-01-01", None)), Holding::Cash(CashKind::Fiat));
        let aapl = asset(72, "aapl", "AAPL", Some("Stock"));
        list(&mut f, &alpaca, Side::Base, "AAPL", &aapl);
        assert_eq!(holding(&f, &leg(&alpaca, Side::Base, "AAPL", "2025-01-01", None)), Holding::Security("aapl".into()));
        let btc = coin_asset(10, "bitcoin", "BTC");
        list(&mut f, &alpaca, Side::Base, "BTC", &btc);
        assert_eq!(holding(&f, &leg(&alpaca, Side::Base, "BTC", "2025-01-01", None)), coin_of("bitcoin"), "broker crypto");
        catalogue(&mut f, &asset(73, "comp-stock", "COMP", Some("Stock")));
        catalogue(&mut f, &coin_asset(74, "compound-governance-token", "COMP"));
        assert_eq!(holding(&f, &leg(&ibkr, Side::Base, "COMP", "2025-01-01", None)), Holding::Unknown(Why::Unclear), "two candidates of two types");
    }

    /// Step 3: a recorded Tokenized Stock or uncategorised asset bound by the venue is that asset; a recorded asset the
    /// venue never bound falls through, a conflict only when the listing names another coin.
    #[test]
    fn a_recorded_asset_counts_when_this_venue_binds_it() {
        let v = venue(3, "kraken", false);
        let mut f = Facts::default();
        let tsla = asset(80, "tesla-xstock", "TSLAX", Some("Tokenized Stock"));
        list(&mut f, &v, Side::Base, "TSLAX", &tsla);
        assert_eq!(holding(&f, &leg(&v, Side::Base, "TSLAX", "2025-06-01", Some(&tsla))), coin_of("tesla-xstock"));
        let cfg = asset(81, "centrifuge", "CFG", None);
        f.stale.entry((v.id, "CFG".into())).or_default().push(cfg.clone());
        let wcfg = coin_asset(82, "wrapped-centrifuge", "WCFG");
        list(&mut f, &v, Side::Base, "CFG", &wcfg);
        assert_eq!(holding(&f, &leg(&v, Side::Base, "CFG", "2023-01-01", Some(&cfg))), coin_of("centrifuge"), "bound by a stale ticker");
        let lone = coin_asset(83, "lonely", "LONE");
        assert_eq!(holding(&f, &leg(&v, Side::Base, "LONE", "2023-01-01", Some(&lone))), Holding::Unknown(Why::Unclear), "unbound, and nothing else says");
        let other = coin_asset(84, "other-lone", "LONE");
        list(&mut f, &v, Side::Base, "LONE", &other);
        assert_eq!(holding(&f, &leg(&v, Side::Base, "LONE", "2023-01-01", Some(&lone))), Holding::Unknown(Why::Conflict), "the listing names another coin");
    }

    /// Codex P1-2/P1-3 (r9, r10): a recorded or listed answer dated inside an interval in which this venue bound the
    /// spelling to another asset is a conflict; outside it, the answer stands. An open-ended alias is held to it too.
    #[test]
    fn alias_contradicted_by_observed_binding() {
        let binance = venue(1, "binance", false);
        let mut f = Facts::default();
        let gold = asset(32, "bitcoin-gold", "BTG", None);
        let open = coin_asset(31, "openverse-network", "BTG");
        catalogue(&mut f, &gold);
        catalogue(&mut f, &open);
        seen(&mut f, &binance, Side::Base, "BTG", 31, "2026-11-20", "2026-11-30");
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "BTG", "2026-11-25", None)), Holding::Unknown(Why::Conflict), "the alias, contradicted");
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "BTG", "2019-01-01", None)), coin_of("bitcoin-gold"), "before the observation");
        // A recorded leg bound by today's listing but dated inside an older, different interval.
        let (x, y) = (coin_asset(50, "coin-x", "S"), coin_asset(51, "coin-y", "S"));
        list(&mut f, &binance, Side::Base, "S", &x);
        seen(&mut f, &binance, Side::Base, "S", 51, "2026-01-01", "2026-03-01");
        seen(&mut f, &binance, Side::Base, "S", 50, "2026-03-02", "2026-11-30");
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "S", "2026-02-01", Some(&x))), Holding::Unknown(Why::Conflict));
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "S", "2026-05-01", Some(&x))), coin_of("coin-x"));
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "S", "2026-02-01", None)), Holding::Unknown(Why::Conflict), "an unrecorded listed leg too");
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "S", "2026-02-01", Some(&y))), coin_of("coin-y"), "the binding observed then");
    }

    /// Step 3b: live before stale, one coin only (MEXC LONG), a live/stale disagreement settled only by an observed
    /// binding; Kraken's own-symbol match last.
    #[test]
    fn a_listing_names_one_coin() {
        let (mexc, kraken) = (venue(2, "mexc", false), venue(3, "kraken", false));
        let mut f = Facts::default();
        list(&mut f, &mexc, Side::Base, "LONG", &coin_asset(60, "long-a", "LONG"));
        list(&mut f, &mexc, Side::Base, "LONG", &coin_asset(61, "long-b", "LONG"));
        assert_eq!(holding(&f, &leg(&mexc, Side::Base, "LONG", "2025-01-01", None)), Holding::Unknown(Why::Unclear));
        let (new, old) = (coin_asset(62, "new-s", "NS"), coin_asset(63, "old-s", "OS"));
        list(&mut f, &mexc, Side::Base, "S", &new);
        f.stale.entry((mexc.id, "S".into())).or_default().push(old.clone());
        assert_eq!(holding(&f, &leg(&mexc, Side::Base, "S", "2024-01-01", None)), Holding::Unknown(Why::Unclear), "live and stale disagree");
        seen(&mut f, &mexc, Side::Base, "S", 63, "2023-12-01", "2024-02-01");
        assert_eq!(holding(&f, &leg(&mexc, Side::Base, "S", "2024-01-01", None)), coin_of("old-s"), "the observed binding dates it");
        let doge = coin_asset(64, "dogecoin", "DOGE");
        list(&mut f, &kraken, Side::Base, "XDG", &doge);
        assert_eq!(holding(&f, &leg(&kraken, Side::Base, "DOGE", "2024-01-01", None)), coin_of("dogecoin"), "Kraken writes the asset's own symbol");
        let eth = coin_asset(65, "ethereum", "ETH");
        list(&mut f, &mexc, Side::Quote, "ETH", &eth);
        assert_eq!(holding(&f, &leg(&mexc, Side::Quote, "ETH", "2024-01-01", None)), coin_of("ethereum"), "a quote leg by tickers.quote_asset_id");
    }

    /// Codex P2 (r10): a fee leg has no ticker side, so its listing inspects both sides and needs one coin; unclear is not a
    /// hard refusal.
    #[test]
    fn hyperliquid_third_token_fee_both_sides() {
        let v = venue(7, "hyperliquid", false);
        let mut f = Facts::default();
        let hype = coin_asset(90, "hyperliquid", "HYPE");
        list(&mut f, &v, Side::Base, "HYPE", &hype);
        assert_eq!(holding(&f, &leg(&v, Side::Fee, "HYPE", "2025-01-01", None)), coin_of("hyperliquid"), "base side only");
        list(&mut f, &v, Side::Quote, "HYPE", &hype);
        assert_eq!(holding(&f, &leg(&v, Side::Fee, "HYPE", "2025-01-01", None)), coin_of("hyperliquid"), "both sides agree");
        list(&mut f, &v, Side::Quote, "HYPE", &coin_asset(91, "hype-other", "HYPE"));
        assert_eq!(holding(&f, &leg(&v, Side::Fee, "HYPE", "2025-01-01", None)), Holding::Unknown(Why::Unclear), "the sides disagree");
        assert!(!holding(&f, &leg(&v, Side::Fee, "HYPE", "2025-01-01", None)).hard());
    }

    /// Step 4 under V: the one non-security asset of a symbol, when it is a coin (XMR, delisted from Binance); none when a
    /// NULL or Currency twin makes two (BTG on KuCoin), or the only one is not a coin.
    #[test]
    fn catalogue_v_keeps_unique_coins_and_refuses_twins() {
        let v = venue(1, "binance", false);
        let mut f = Facts::default();
        catalogue(&mut f, &coin_asset(100, "monero", "XMR"));
        catalogue(&mut f, &asset(101, "xmr-stock", "XMR", Some("Stock")));
        assert_eq!(holding(&f, &leg(&v, Side::Base, "XMR", "2025-01-01", None)), coin_of("monero"), "securities do not count");
        catalogue(&mut f, &asset(102, "only-fiat", "OFT", Some("Currency")));
        assert_eq!(holding(&f, &leg(&v, Side::Base, "OFT", "2025-01-01", None)), Holding::Unknown(Why::Unclear));
        assert_eq!(holding(&f, &leg(&v, Side::Base, "NONE", "2025-01-01", None)), Holding::Unknown(Why::Unclear));
        assert_eq!(CATALOGUE, Catalogue::V, "the plan's recommendation until the owner decides");
    }
}
