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
//! A spelling the venue has rebound (`rebound`: its live and tombstoned listings name two assets) overrides steps 2–4:
//! a recorded leg keeps its recorded coin only when that coin is among the venue's listings of the spelling, else
//! `Unknown(Conflict)`; an unrecorded leg is `Unknown(Unclear)`. There is no dated evidence (plan r11): what the
//! tickers table says now is all the resolver reads.
use super::{fiat, stable};
use chrono::{DateTime, NaiveDate, Utc};
use std::collections::HashMap;

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
    fn currency(&self) -> bool { matches!(self.category.as_deref(), Some("Fiat" | "Currency")) }
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

/// What the resolver reads, indexed in memory (PR1 loads it; PR0 tests build it by hand).
#[derive(Default, Clone, Debug)]
pub struct Facts {
    /// (venue, side, spelling) → the assets its live tickers bind to it, in ticker order.
    pub live: HashMap<(i64, Side, String), Vec<AssetRef>>,
    /// (venue, spelling) → the assets its tombstoned tickers (`__stale_<id>_<spelling>`) bound as a base.
    pub stale: HashMap<(i64, String), Vec<AssetRef>>,
    /// symbol → every catalogue asset with it.
    pub catalogue: HashMap<String, Vec<AssetRef>>,
}
impl Facts {
    /// Each list's first listing of each asset, in order: every reader takes the assets `distinct`, so the answers stay
    /// as they were, and a leg no longer walks a broker's whole quote pool (Alpaca lists every symbol in USD).
    pub fn dedup(&mut self) {
        for list in self.live.values_mut().chain(self.stale.values_mut()).chain(self.catalogue.values_mut()) {
            let mut seen = std::collections::HashSet::new();
            list.retain(|a| seen.insert(a.id));
        }
    }
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

/// A coin answer for `leg`: refused under another unit's spelling.
fn coin(leg: &Leg, asset: &AssetRef) -> Holding {
    if asset.symbol.as_deref().is_some_and(|s| other_units(leg.spelling, s)) { return Holding::Unknown(Why::Units); }
    asset.external_id.clone().map_or(Holding::Unknown(Why::Unclear), Holding::Coin)
}

/// The assets this venue's live base tickers and tombstoned ones bind to `spelling`, each once.
fn base_listings(f: &Facts, venue: i64, spelling: &str) -> Vec<AssetRef> {
    distinct(f.live.get(&(venue, Side::Base, spelling.to_string())).into_iter().chain(f.stale.get(&(venue, spelling.to_string()))).flatten().cloned())
}

/// Whether the venue has rebound `spelling`: a tombstone of it, and its live and tombstoned listings name more than one
/// asset. (A tombstone of the same asset is a pair renamed, not a spelling reused.) PR1's venue-candle routes refuse a
/// coin candle for a rebound spelling: such a spelling gets no venue-candle fallback, which costs coverage, never a
/// wrong price.
pub fn rebound(f: &Facts, venue: i64, spelling: &str) -> bool {
    f.stale.get(&(venue, spelling.to_string())).is_some_and(|s| !s.is_empty()) && base_listings(f, venue, spelling).len() > 1
}

/// Step 2: a cut or a dated alias of this venue, in its window.
fn aliased(leg: &Leg) -> Option<Holding> {
    ALIASES_V2.iter().find(|(s, _, v, until)| *s == leg.spelling && v.is_none_or(|v| v == leg.venue.name_id) && within(*until, leg.at))
        .map(|(_, ext, _, _)| Holding::Coin(ext.to_string()))
}

/// Whether this venue's listings (live or stale) bind `asset` to the base spelling.
fn bound(f: &Facts, leg: &Leg, asset: &AssetRef) -> bool { base_listings(f, leg.venue.id, leg.spelling).iter().any(|a| a.id == asset.id) }

fn distinct(assets: impl IntoIterator<Item = AssetRef>) -> Vec<AssetRef> {
    let mut out: Vec<AssetRef> = vec![];
    for a in assets { if !out.iter().any(|o| o.id == a.id) { out.push(a); } }
    out
}

/// Step 3b: this venue's listing of the spelling on the leg's side(s), coins only: exact live, else stale (base only),
/// else Kraken's own-symbol match. One coin, else unclear (a rebound spelling never reaches here).
fn listed(f: &Facts, leg: &Leg) -> Option<Holding> {
    // Each side on its own (a fee reads both, and needs them to agree).
    let answers: Vec<Holding> = sides(leg).iter().filter_map(|side| listed_on(f, leg, *side)).collect();
    // Sides that disagree name two holdings: a conflict, which inherits no lot (`agreed`).
    agreed(&answers)
}

/// What a leg's listing sides say together: the one answer they share; two definite answers are a conflict (hard: a
/// fee in złoty never inherits the PLEARN lot booked under `PLN`); a side's hard refusal stands; a side otherwise
/// unclear leaves the leg unclear.
fn agreed(answers: &[Holding]) -> Option<Holding> {
    match answers {
        [] => None,
        [a, rest @ ..] if rest.iter().all(|b| b == a) => Some(a.clone()),
        all => Some(all.iter().find(|a| a.hard()).cloned().unwrap_or(Holding::Unknown(
            if all.iter().any(|a| matches!(a, Holding::Unknown(_))) { Why::Unclear } else { Why::Conflict }))),
    }
}

/// One side's listing of the spelling: exact live, else stale (base), else Kraken's own-symbol match; coins, and under
/// a fiat spelling the currency itself (a venue listing PLEARN as base `PLN` and the złoty as quote `PLN`: a `PLN` fee
/// has two answers).
fn listed_on(f: &Facts, leg: &Leg, side: Side) -> Option<Holding> {
    let coins = |v: Option<&Vec<AssetRef>>| distinct(v.into_iter().flatten().filter(|a| a.crypto() || (fiat(leg.spelling) && a.currency())).cloned());
    let live = coins(f.live.get(&(leg.venue.id, side, leg.spelling.to_string())));
    let stale = if side == Side::Base { coins(f.stale.get(&(leg.venue.id, leg.spelling.to_string()))) } else { vec![] };
    let own = || {
        let all = f.live.iter().filter(|((v, s, _), _)| *v == leg.venue.id && *s == side).flat_map(|(_, l)| l);
        distinct(all.filter(|a| a.crypto() && a.symbol.as_deref() == Some(leg.spelling)).cloned())
    };
    let candidates = if !live.is_empty() { live } else if !stale.is_empty() { stale } else if leg.venue.name_id == "kraken" { own() } else { vec![] };
    match candidates.as_slice() {
        [] => None,
        [one] if one.currency() => Some(Holding::Cash(CashKind::Fiat)),
        [one] => Some(coin(leg, one)),
        _ => Some(Holding::Unknown(Why::Unclear)),
    }
}

/// Step 4: the catalogue.
fn catalogued(f: &Facts, leg: &Leg) -> Option<Holding> {
    if CATALOGUE == Catalogue::Strict { return None; }
    let all = distinct(f.catalogue.get(leg.spelling).into_iter().flatten().cloned());
    let others: Vec<&AssetRef> = all.iter().filter(|a| !a.security()).collect();
    match others.as_slice() { [one] if one.crypto() => Some(coin(leg, one)), _ => None }
}

/// The listing sides a leg reads: its own, or both for a fee (which has no ticker side).
fn sides(leg: &Leg) -> &'static [Side] { if leg.side == Side::Fee { &[Side::Base, Side::Quote] } else if leg.side == Side::Base { &[Side::Base] } else { &[Side::Quote] } }

/// A broker's unrecorded leg, and whether the broker's own listings answered: each listing side on its own (both for a
/// fee; live, else the base side's tombstones), needing them to agree; with no listing at all, the catalogue when
/// `catalogue` allows it. Cash assets aside, one security or coin is that holding; several, or one unclassified
/// instrument, are unclear; none is no evidence. For a fiat spelling (`catalogue` false) securities alone are no
/// evidence either: the leg stays cash.
fn broker_unrecorded(f: &Facts, leg: &Leg, catalogue: bool) -> (bool, Option<Holding>) {
    let answer = |pool: Vec<AssetRef>| {
        let held: Vec<&AssetRef> = pool.iter().filter(|a| !a.currency()).collect();
        // Under a fiat spelling the currency itself, listed, is this side's answer (securities beside it are none).
        if fiat(leg.spelling) && held.len() < pool.len() {
            return Some(if held.iter().all(|a| a.security()) { Holding::Cash(CashKind::Fiat) } else { Holding::Unknown(Why::Unclear) });
        }
        if !catalogue && held.iter().all(|a| a.security()) { return None; }
        Some(match held.as_slice() {
            [] => return None,
            [one] if one.security() => one.external_id.clone().map_or(Holding::Unknown(Why::Unclear), Holding::Security),
            [one] if one.crypto() => coin(leg, one),
            _ => Holding::Unknown(Why::Unclear),
        })
    };
    let listings: Vec<Vec<AssetRef>> = sides(leg).iter().map(|side| {
        let live = distinct(f.live.get(&(leg.venue.id, *side, leg.spelling.to_string())).into_iter().flatten().cloned());
        if live.is_empty() && *side == Side::Base { distinct(f.stale.get(&(leg.venue.id, leg.spelling.to_string())).into_iter().flatten().cloned()) } else { live }
    }).filter(|l| !l.is_empty()).collect();
    if listings.is_empty() {
        return (false, if catalogue { answer(distinct(f.catalogue.get(leg.spelling).into_iter().flatten().cloned())) } else { None });
    }
    let answers: Vec<Holding> = listings.into_iter().filter_map(answer).collect();
    // Sides that disagree name two holdings: a conflict, which inherits nothing (an unclear fee would take whatever
    // lot the account books under the spelling, a third instrument included).
    (true, agreed(&answers))
}

/// Steps 2–4 for a leg that is not cash by spelling (or a fiat spelling looking for coin evidence: `catalogue` false).
fn evidence(f: &Facts, leg: &Leg, catalogue: bool) -> Option<Holding> {
    let recorded = leg.recorded.filter(|a| leg.side == Side::Base && (a.crypto() || a.category.is_none() || a.category.as_deref() == Some("Tokenized Stock")));
    if CUTS.iter().any(|(s, until)| *s == leg.spelling && within(*until, leg.at)) { return Some(Holding::Unknown(Why::Cut)); }
    // A rebound spelling: only what the venue listed under it, recorded on the row, is a coin; but not an id sync's own
    // alias could have recorded (it records main's alias whatever the venue listed; never on a broker, whose rows are
    // recorded from its own activity and listings, sync/ledger.rs `asset_ids`), and never for a fiat spelling whose
    // listings are securities alone (they never displace the cash).
    // A fee whose listing sides name two holdings is that conflict whatever else the spelling is (rebound included).
    if leg.side == Side::Fee {
        let sides = if leg.venue.broker { broker_unrecorded(f, leg, catalogue).1 } else { listed(f, leg) };
        if sides == Some(Holding::Unknown(Why::Conflict)) { return sides; }
    }
    let securities_only = base_listings(f, leg.venue.id, leg.spelling).iter().all(|a| a.security() || a.currency());
    if leg.side != Side::Quote && rebound(f, leg.venue.id, leg.spelling) && (catalogue || !securities_only) {
        let by_alias = |r: &AssetRef| {
            let venue = super::prices::Venue { id: leg.venue.id, name_id: leg.venue.name_id.clone(), stock: leg.venue.broker };
            !leg.venue.broker && r.external_id.is_some() && super::prices::alias_coin(leg.spelling, Some(&venue), Some(leg.at.date_naive())) == r.external_id
        };
        return Some(match recorded {
            Some(r) if by_alias(r) => Holding::Unknown(Why::Unclear),
            Some(r) if bound(f, leg, r) => coin(leg, r),
            Some(_) => Holding::Unknown(Why::Conflict),
            None => Holding::Unknown(Why::Unclear),
        });
    }
    // A broker's own instruments before any crypto alias: what its listings say stands, unclear included (an IBKR LUNA
    // stock is the stock; a stock and a coin listed under LUNA are unclear), and so does a recorded coin they bind.
    // With no listing, only a catalogue security comes before the alias.
    if leg.venue.broker {
        match recorded {
            Some(r) if bound(f, leg, r) => return Some(coin(leg, r)),
            // Recorded, unbound: the broker's listing stands, and another coin listed is a conflict (never the alias).
            Some(r) => if let (true, Some(h)) = broker_unrecorded(f, leg, catalogue) {
                return Some(match h {
                    Holding::Coin(other) if r.external_id.as_deref() != Some(other.as_str()) => Holding::Unknown(Why::Conflict),
                    // A recorded cryptocurrency is never the security listed under its spelling (Alpaca's BTC, an ETF's tombstone).
                    Holding::Security(_) if r.crypto() => Holding::Unknown(Why::Conflict),
                    h => h,
                });
            } else if r.crypto() {
                // No listing of the broker's says otherwise: it recorded the fill from its own activity, so the coin it
                // recorded stands (the catalogue's BTC coin and BTC ETF are no evidence against it).
                return Some(coin(leg, r));
            },
            None => match broker_unrecorded(f, leg, catalogue) {
                // Alpaca's row from a file under a name its curated map trades as a coin: sync leaves it unrecorded
                // because it could be either (sync/ledger.rs `asset_id`), so the security listed is not an answer.
                (_, Some(Holding::Security(_))) if leg.recorded.is_none() && leg.side == Side::Base && leg.venue.name_id == "alpaca"
                    && crate::sync::ledger::CRYPTO_COINGECKO_IDS.iter().any(|(k, _)| *k == leg.spelling) => return Some(Holding::Unknown(Why::Unclear)),
                (true, Some(h)) | (false, Some(h @ Holding::Security(_))) => return Some(h),
                _ => {}
            },
        }
    }
    if let Some(h) = aliased(leg) { return Some(h); }
    if let Some(recorded) = recorded {
        if bound(f, leg, recorded) { return Some(coin(leg, recorded)); }
        // Not bound here: what the venue's listing (or the catalogue) says, a conflict when it names another coin.
        let rest = if leg.venue.broker { broker_unrecorded(f, leg, catalogue).1 } else { listed(f, leg).or_else(|| if catalogue { catalogued(f, leg) } else { None }) };
        return match rest {
            Some(Holding::Coin(other)) if recorded.external_id.as_deref() != Some(other.as_str()) => Some(Holding::Unknown(Why::Conflict)),
            other => other,
        };
    }
    if leg.venue.broker { return broker_unrecorded(f, leg, catalogue).1; }
    listed(f, leg).or_else(|| if catalogue { catalogued(f, leg) } else { None })
}

/// What `leg` holds (§1.1).
pub fn holding(f: &Facts, leg: &Leg) -> Holding {
    if let Some(recorded) = leg.recorded.filter(|a| leg.venue.broker && leg.side == Side::Base && a.security()) {
        if let Some(id) = &recorded.external_id { return Holding::Security(id.clone()); }
    }
    if stable(leg.spelling) { return Holding::Cash(CashKind::Stable); }
    if fiat(leg.spelling) {
        // This venue's coin evidence for the leg wins, a refusal or an unclear answer included; a listed security never
        // makes a fiat spelling a security (Alpaca's USD.US under `USD`): only a recorded one does (above).
        return match evidence(f, leg, false) { Some(h @ (Holding::Coin(_) | Holding::Unknown(_))) => h, _ => Holding::Cash(CashKind::Fiat) };
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
        // Codex PR1a r5: a fee's other side naming another coin never softens the refusal.
        list(&mut f, &v, Side::Quote, "1000REKT", &coin_asset(77, "thousand-rekt", "1000REKT"));
        assert_eq!(holding(&f, &leg(&v, Side::Fee, "1000REKT", "2025-01-01", None)), Holding::Unknown(Why::Units));
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
    /// Plan r11: a spelling the venue rebound (live and tombstoned listings name two assets) beats the alias: a recorded
    /// leg keeps its coin only if the venue listed it under the spelling, else a conflict; an unrecorded one is unclear.
    /// A tombstone of the same asset (a pair renamed) is no rebinding.
    #[test]
    fn a_rebound_spelling_keeps_only_recorded_coins_it_listed() {
        let binance = venue(1, "binance", false);
        let mut f = Facts::default();
        let (gold, open) = (asset(32, "bitcoin-gold", "BTG", None), coin_asset(31, "openverse-network", "BTG"));
        catalogue(&mut f, &gold);
        catalogue(&mut f, &open);
        assert!(!rebound(&f, binance.id, "BTG"));
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "BTG", "2019-01-01", None)), coin_of("bitcoin-gold"), "not rebound: the alias");
        list(&mut f, &binance, Side::Base, "BTG", &open);
        f.stale.entry((binance.id, "BTG".into())).or_default().push(gold.clone());
        assert!(rebound(&f, binance.id, "BTG"));
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "BTG", "2019-01-01", None)), Holding::Unknown(Why::Unclear));
        assert_eq!(holding(&f, &leg(&binance, Side::Fee, "BTG", "2019-01-01", None)), Holding::Unknown(Why::Unclear), "a fee too");
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "BTG", "2019-01-01", Some(&gold))), coin_of("bitcoin-gold"));
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "BTG", "2026-01-01", Some(&open))), coin_of("openverse-network"));
        let stray = coin_asset(33, "stray", "BTG");
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "BTG", "2026-01-01", Some(&stray))), Holding::Unknown(Why::Conflict));
        let x = coin_asset(50, "coin-x", "S");
        list(&mut f, &binance, Side::Base, "S", &x);
        f.stale.entry((binance.id, "S".into())).or_default().push(x.clone());
        assert!(!rebound(&f, binance.id, "S"), "a tombstone of the same asset");
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "S", "2026-01-01", None)), coin_of("coin-x"));
    }

    /// Step 3b: live before stale, one coin only (MEXC LONG), a live/stale disagreement (a rebound spelling) unclear;
    /// Kraken's own-symbol match last.
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

    /// Codex PR0 P1-6: a hard refusal under a fiat spelling stays a refusal (a recorded coin MEXC never listed under its
    /// rebound PLN is a conflict, never the złoty), and an unrecorded leg there is unclear; a broker's
    /// unlisted SEK never takes a lone catalogue coin, its USD quote is cash; a broker's fee reads both listing sides.
    #[test]
    fn fiat_spellings_keep_refusals_and_brokers_keep_the_catalogue_rule() {
        let (mexc, alpaca) = (venue(2, "mexc", false), venue(5, "alpaca", true));
        let mut f = Facts::default();
        let plearn = coin_asset(2227, "pleasure-coin", "PLN");
        list(&mut f, &mexc, Side::Base, "PLN", &plearn);
        f.stale.entry((mexc.id, "PLN".into())).or_default().push(coin_asset(2228, "old-pln", "PLN"));
        assert_eq!(holding(&f, &leg(&mexc, Side::Base, "PLN", "2024-01-15", Some(&coin_asset(2229, "third", "PLN")))), Holding::Unknown(Why::Conflict), "rebound, unlisted");
        assert_eq!(holding(&f, &leg(&mexc, Side::Base, "PLN", "2024-01-15", Some(&plearn))), coin_of("pleasure-coin"));
        assert_eq!(holding(&f, &leg(&mexc, Side::Base, "PLN", "2025-01-01", None)), Holding::Unknown(Why::Unclear), "rebound: not the złoty");
        catalogue(&mut f, &coin_asset(900, "sek-coin", "SEK"));
        assert_eq!(holding(&f, &leg(&alpaca, Side::Base, "SEK", "2025-01-01", None)), Holding::Cash(CashKind::Fiat));
        list(&mut f, &alpaca, Side::Quote, "USD", &asset(1, "usd", "USD", Some("Fiat")));
        assert_eq!(holding(&f, &leg(&alpaca, Side::Quote, "USD", "2025-01-01", None)), Holding::Cash(CashKind::Fiat));
        list(&mut f, &alpaca, Side::Quote, "BTC", &coin_asset(10, "bitcoin", "BTC"));
        assert_eq!(holding(&f, &leg(&alpaca, Side::Fee, "BTC", "2025-01-01", None)), coin_of("bitcoin"), "a broker's fee reads the quote side too");
    }

    /// Codex PR0 r2 P2-10: on a broker, a fiat spelling yields only to coin evidence or a refusal: Alpaca listing the
    /// USD.US stock under the base `USD` leaves an unrecorded USD cash activity cash (a recorded USD.US is still the
    /// security); a coin and an unclassified instrument sharing a spelling are two candidates, not one coin.
    #[test]
    fn broker_cash_and_ambiguous_instruments() {
        let alpaca = venue(5, "alpaca", true);
        let mut f = Facts::default();
        let usd_us = asset(70, "usd-us-stock", "USD", Some("Stock"));
        list(&mut f, &alpaca, Side::Base, "USD", &usd_us);
        assert_eq!(holding(&f, &leg(&alpaca, Side::Base, "USD", "2025-01-01", None)), Holding::Cash(CashKind::Fiat));
        assert_eq!(holding(&f, &leg(&alpaca, Side::Base, "USD", "2025-01-01", Some(&usd_us))), Holding::Security("usd-us-stock".into()));
        list(&mut f, &alpaca, Side::Base, "ZED", &coin_asset(71, "zed-coin", "ZED"));
        list(&mut f, &alpaca, Side::Base, "ZED", &asset(72, "zed-thing", "ZED", None));
        assert_eq!(holding(&f, &leg(&alpaca, Side::Base, "ZED", "2025-01-01", None)), Holding::Unknown(Why::Unclear));
    }

    /// Codex PR0 r3 P2-9: securities alone never displace a broker's fiat cash: IBKR listing two securities under the
    /// base `USD` leaves an unrecorded USD activity cash; a coin among them keeps the answer unclear.
    #[test]
    fn listed_securities_alone_leave_broker_fiat_cash() {
        let ibkr = venue(6, "ibkr", true);
        let mut f = Facts::default();
        list(&mut f, &ibkr, Side::Base, "USD", &asset(80, "usd-a", "USD", Some("Stock")));
        list(&mut f, &ibkr, Side::Base, "USD", &asset(81, "usd-b", "USD", Some("ETF")));
        assert_eq!(holding(&f, &leg(&ibkr, Side::Base, "USD", "2025-01-01", None)), Holding::Cash(CashKind::Fiat));
        list(&mut f, &ibkr, Side::Base, "USD", &coin_asset(82, "usd-coin-thing", "USD"));
        assert_eq!(holding(&f, &leg(&ibkr, Side::Base, "USD", "2025-01-01", None)), Holding::Unknown(Why::Unclear));
    }

    /// Codex PR0 r4 P1: sync records main's open-ended alias coin (Binance GAL → project-galaxy) whatever the venue lists,
    /// so on a rebound GAL that recorded id is no evidence: the leg is unclear. Another recorded coin the venue listed
    /// stands.
    #[test]
    fn a_recorded_alias_coin_on_a_rebound_spelling_is_no_evidence() {
        let binance = venue(1, "binance", false);
        let mut f = Facts::default();
        let (old, new) = (coin_asset(40, "project-galaxy", "GAL"), coin_asset(41, "gal-new", "GAL"));
        list(&mut f, &binance, Side::Base, "GAL", &new);
        f.stale.entry((binance.id, "GAL".into())).or_default().push(old.clone());
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "GAL", "2026-01-01", Some(&old))), Holding::Unknown(Why::Unclear));
        assert_eq!(holding(&f, &leg(&binance, Side::Base, "GAL", "2026-01-01", Some(&new))), coin_of("gal-new"));
    }

    /// Codex PR0 r4 P2: a fee leg resolves each side on its own and needs them to agree: a tombstoned base X and a live
    /// quote Y are two answers.
    #[test]
    fn a_fee_resolves_each_side_and_needs_them_to_agree() {
        let v = venue(2, "mexc", false);
        let mut f = Facts::default();
        f.stale.entry((v.id, "S".into())).or_default().push(coin_asset(50, "coin-x", "S"));
        list(&mut f, &v, Side::Quote, "S", &coin_asset(51, "coin-y", "S"));
        assert_eq!(holding(&f, &leg(&v, Side::Fee, "S", "2025-01-01", None)), Holding::Unknown(Why::Conflict), "two answers: inherits no lot");
        assert_eq!(holding(&f, &leg(&v, Side::Quote, "S", "2025-01-01", None)), coin_of("coin-y"));
    }

    /// Codex PR0 r4 P2: an unrecorded broker leg reads the broker's instruments before a crypto alias (an IBKR LUNA stock
    /// before 28 May 2022 is the stock); a securities-only tombstone under a broker's USD leaves USD cash.
    #[test]
    fn broker_instruments_come_before_aliases_and_securities_never_rebind_cash() {
        let (ibkr, alpaca) = (venue(6, "ibkr", true), venue(5, "alpaca", true));
        let mut f = Facts::default();
        list(&mut f, &ibkr, Side::Base, "LUNA", &asset(90, "luna-stock", "LUNA", Some("Stock")));
        assert_eq!(holding(&f, &leg(&ibkr, Side::Base, "LUNA", "2022-01-01", None)), Holding::Security("luna-stock".into()));
        list(&mut f, &alpaca, Side::Base, "USD", &asset(91, "usd-a", "USD", Some("Stock")));
        f.stale.entry((alpaca.id, "USD".into())).or_default().push(asset(92, "usd-b", "USD", Some("Stock")));
        assert_eq!(holding(&f, &leg(&alpaca, Side::Base, "USD", "2025-01-01", None)), Holding::Cash(CashKind::Fiat));
    }

    /// Codex PR0 r5 P2: a broker listing a stock and a coin under LUNA keeps an unrecorded trade unclear before the
    /// alias's last day (the alias is no broker evidence); a broker's sole listed coin stands against the alias too.
    #[test]
    fn a_brokers_ambiguous_listing_beats_the_alias() {
        let (ibkr, alpaca) = (venue(6, "ibkr", true), venue(5, "alpaca", true));
        let mut f = Facts::default();
        list(&mut f, &ibkr, Side::Base, "LUNA", &asset(90, "luna-stock", "LUNA", Some("Stock")));
        list(&mut f, &ibkr, Side::Base, "LUNA", &coin_asset(91, "terra-luna-2", "LUNA"));
        assert_eq!(holding(&f, &leg(&ibkr, Side::Base, "LUNA", "2022-01-01", None)), Holding::Unknown(Why::Unclear));
        list(&mut f, &alpaca, Side::Base, "LUNA", &coin_asset(91, "terra-luna-2", "LUNA"));
        assert_eq!(holding(&f, &leg(&alpaca, Side::Base, "LUNA", "2022-01-01", None)), coin_of("terra-luna-2"), "what the broker lists");
        let unlisted = venue(8, "ibkr", true);
        catalogue(&mut f, &coin_asset(91, "terra-luna-2", "LUNA"));
        assert_eq!(holding(&f, &leg(&unlisted, Side::Base, "LUNA", "2022-01-01", None)), coin_of("terra-luna"), "a catalogue coin yields to the alias");
    }

    /// Codex PR0 r5 P2: a broker's fee reads each listing side on its own, the base side's tombstones included: a
    /// tombstoned base X and a live quote Y are two answers.
    #[test]
    fn a_brokers_fee_keeps_tombstoned_base_evidence() {
        let alpaca = venue(5, "alpaca", true);
        let mut f = Facts::default();
        f.stale.entry((alpaca.id, "S".into())).or_default().push(coin_asset(50, "coin-x", "S"));
        list(&mut f, &alpaca, Side::Quote, "S", &coin_asset(51, "coin-y", "S"));
        assert_eq!(holding(&f, &leg(&alpaca, Side::Fee, "S", "2025-01-01", None)), Holding::Unknown(Why::Conflict), "two sides, two coins: inherits nothing");
        assert_eq!(holding(&f, &leg(&alpaca, Side::Base, "S", "2025-01-01", None)), coin_of("coin-x"), "the base leg: its tombstone");
    }

    /// Codex PR0 r5 P2: a venue listing PLEARN as base `PLN` and the złoty as quote `PLN`: each leg is what its side
    /// lists, and a `PLN` fee, with two answers, is unclear (a fee paid in złoty never spends the coin).
    #[test]
    fn an_explicit_fiat_quote_is_a_fee_sides_answer() {
        let v = venue(2, "mexc", false);
        let mut f = Facts::default();
        list(&mut f, &v, Side::Base, "PLN", &coin_asset(2227, "pleasure-coin", "PLN"));
        list(&mut f, &v, Side::Quote, "PLN", &asset(3, "pln", "PLN", Some("Fiat")));
        assert_eq!(holding(&f, &leg(&v, Side::Base, "PLN", "2025-01-01", None)), coin_of("pleasure-coin"));
        assert_eq!(holding(&f, &leg(&v, Side::Quote, "PLN", "2025-01-01", None)), Holding::Cash(CashKind::Fiat));
        assert_eq!(holding(&f, &leg(&v, Side::Fee, "PLN", "2025-01-01", None)), Holding::Unknown(Why::Conflict), "inherits no lot");
        // Codex PR1a r4: the conflict stands on a rebound spelling too.
        f.stale.entry((v.id, "PLN".into())).or_default().push(coin_asset(2226, "old-pln-coin", "PLN"));
        assert_eq!(holding(&f, &leg(&v, Side::Fee, "PLN", "2025-01-01", None)), Holding::Unknown(Why::Conflict));
        let other = venue(3, "kraken", false);
        list(&mut f, &other, Side::Quote, "PLN", &asset(3, "pln", "PLN", Some("Fiat")));
        assert_eq!(holding(&f, &leg(&other, Side::Fee, "PLN", "2025-01-01", None)), Holding::Cash(CashKind::Fiat), "the złoty alone");
    }

    /// Codex PR0 r5 P2: a broker records its fills from its own activity and listings, never through the alias: on a
    /// rebound Alpaca MATIC, a fill recorded as matic-network and bound by the tombstone keeps its coin.
    #[test]
    fn a_brokers_recorded_coin_on_a_rebound_spelling_stands() {
        let alpaca = venue(5, "alpaca", true);
        let mut f = Facts::default();
        let (old, new) = (coin_asset(40, "matic-network", "MATIC"), coin_asset(41, "matic-new", "MATIC"));
        list(&mut f, &alpaca, Side::Base, "MATIC", &new);
        f.stale.entry((alpaca.id, "MATIC".into())).or_default().push(old.clone());
        assert_eq!(holding(&f, &leg(&alpaca, Side::Base, "MATIC", "2023-01-01", Some(&old))), coin_of("matic-network"));
    }

    /// Codex PR1a r1 P2: a broker's explicit fiat listing is a fee side's answer too; with no broker listing, catalogue
    /// ambiguity never beats a dated alias; a recorded coin the broker's listing binds stands against an alias.
    #[test]
    fn broker_fiat_sides_catalogue_ambiguity_and_bound_recorded_coins() {
        let (alpaca, ibkr) = (venue(5, "alpaca", true), venue(6, "ibkr", true));
        let mut f = Facts::default();
        list(&mut f, &alpaca, Side::Base, "PLN", &coin_asset(2227, "pleasure-coin", "PLN"));
        list(&mut f, &alpaca, Side::Quote, "PLN", &asset(3, "pln", "PLN", Some("Fiat")));
        assert_eq!(holding(&f, &leg(&alpaca, Side::Fee, "PLN", "2025-01-01", None)), Holding::Unknown(Why::Conflict));
        assert_eq!(holding(&f, &leg(&alpaca, Side::Quote, "PLN", "2025-01-01", None)), Holding::Cash(CashKind::Fiat));
        assert_eq!(holding(&f, &leg(&alpaca, Side::Base, "PLN", "2025-01-01", None)), coin_of("pleasure-coin"));
        catalogue(&mut f, &coin_asset(91, "terra-luna-2", "LUNA"));
        catalogue(&mut f, &coin_asset(92, "luna-other", "LUNA"));
        assert_eq!(holding(&f, &leg(&ibkr, Side::Base, "LUNA", "2022-01-01", None)), coin_of("terra-luna"), "no listing: the alias");
        let x = coin_asset(41, "matic-x", "MATIC");
        list(&mut f, &alpaca, Side::Base, "MATIC", &x);
        assert_eq!(holding(&f, &leg(&alpaca, Side::Base, "MATIC", "2025-01-01", None)), coin_of("matic-x"));
        assert_eq!(holding(&f, &leg(&alpaca, Side::Base, "MATIC", "2025-01-01", Some(&x))), coin_of("matic-x"), "recorded and bound: the same coin");
        let y = coin_asset(42, "matic-y", "MATIC");
        assert_eq!(holding(&f, &leg(&alpaca, Side::Base, "MATIC", "2025-01-01", Some(&y))), Holding::Unknown(Why::Conflict), "recorded Y, listed X: never the alias");
        // Codex PR1a r3: a recorded cryptocurrency against the tombstone of an ETF under its spelling is neither.
        f.stale.entry((alpaca.id, "BTC".into())).or_default().push(asset(60, "BTC.ARCX", "BTC", Some("ETF")));
        assert_eq!(holding(&f, &leg(&alpaca, Side::Base, "BTC", "2025-01-01", Some(&coin_asset(1, "bitcoin", "BTC")))), Holding::Unknown(Why::Conflict));
        // Codex PR1a r4: unrecorded (a file's row), under a name Alpaca's curated map trades as a coin: either.
        assert_eq!(holding(&f, &leg(&alpaca, Side::Base, "BTC", "2025-01-01", None)), Holding::Unknown(Why::Unclear));
        // Recorded as the coin, with no listing of the broker's under BTC: the coin, though the catalogue has the ETF too.
        let bare = venue(9, "alpaca", true);
        let mut g = Facts::default();
        catalogue(&mut g, &asset(60, "BTC.ARCX", "BTC", Some("ETF")));
        catalogue(&mut g, &coin_asset(1, "bitcoin", "BTC"));
        assert_eq!(holding(&g, &leg(&bare, Side::Base, "BTC", "2025-01-01", Some(&coin_asset(1, "bitcoin", "BTC")))), coin_of("bitcoin"));
        let etf = asset(60, "BTC.ARCX", "BTC", Some("ETF"));
        assert_eq!(holding(&f, &leg(&alpaca, Side::Base, "BTC", "2025-01-01", Some(&etf))), Holding::Security("BTC.ARCX".into()), "recorded as the ETF");
    }

    /// A broker lists thousands of tickers under one quote (Alpaca: every symbol in USD, one asset), and every leg read
    /// that pool whole: `dedup` keeps each list's first listing of each asset, in order, and every answer as it was.
    #[test]
    fn dedup_keeps_each_assets_first_listing_and_every_answer() {
        let alpaca = venue(3, "alpaca", true);
        let (usd, spy, voo) = (asset(1, "usd", "USD", Some("Fiat")), asset(50, "SPY", "SPY", Some("ETF")), asset(51, "VOO", "VOO", Some("ETF")));
        let mut f = Facts::default();
        for _ in 0..5_000 { list(&mut f, &alpaca, Side::Quote, "USD", &usd); }
        for a in [&spy, &voo, &spy, &voo] { list(&mut f, &alpaca, Side::Base, "SPY", a); }
        f.stale.entry((alpaca.id, "SPY".into())).or_default().extend([voo.clone(), voo.clone()]);
        for a in [&spy, &spy] { catalogue(&mut f, a); }
        let legs = [leg(&alpaca, Side::Quote, "USD", "2025-01-01", None), leg(&alpaca, Side::Base, "SPY", "2025-01-01", None),
                    leg(&alpaca, Side::Fee, "USD", "2025-01-01", None), leg(&alpaca, Side::Base, "SPY", "2025-01-01", Some(&spy))];
        let before: Vec<Holding> = legs.iter().map(|l| holding(&f, l)).collect();
        f.dedup();
        let ids = |v: &Vec<AssetRef>| v.iter().map(|a| a.id).collect::<Vec<_>>();
        assert_eq!(ids(&f.live[&(alpaca.id, Side::Quote, "USD".to_string())]), [1]);
        assert_eq!(ids(&f.live[&(alpaca.id, Side::Base, "SPY".to_string())]), [50, 51]);
        assert_eq!(ids(&f.stale[&(alpaca.id, "SPY".to_string())]), [51]);
        assert_eq!(ids(&f.catalogue["SPY"]), [50]);
        assert_eq!(legs.iter().map(|l| holding(&f, l)).collect::<Vec<_>>(), before);
    }
}
