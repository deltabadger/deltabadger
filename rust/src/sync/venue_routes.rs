//! Routes from a held asset to dollars over a venue's own priced pairs (Rust only, PLAN-venue-prices S2,
//! D-VENUE-PRICE-FALLBACK): the provider's fallback for a balance it left unpriced. Pure: the pairs, their last prices
//! and the fiat ends come in; candidate dollar prices go out, best first.
//!
//! A route walks at most `MAX_LEGS` pairs, each one way or the other (base→quote at its price, quote→base at the
//! inverse), through any assets, to a fiat end: USD, or a fiat with a rate to USD (the ECB's). A stablecoin is never an
//! end: it is priced by its own route like any coin. Only pairs with a positive price are walked.
use crate::ruby::BigDec;
use std::collections::{BTreeMap, HashMap};

pub const MAX_LEGS: usize = 3;
/// Candidates returned per length: enough to fall past an unusable product, few enough to stay cheap.
pub const MAX_CANDIDATES: usize = 64;
/// Partial routes expanded per length, whatever the venue's size.
const MAX_EXPANSIONS: usize = 20_000;

/// One listed pair: its code (`tickers.ticker`), and its base and quote assets.
#[derive(Clone, Debug)]
pub struct Pair { pub id: i64, pub code: String, pub base: i64, pub quote: i64 }

/// A fiat end: whether it is the dollar, and its rate to USD (1 for the dollar).
#[derive(Clone, Debug)]
pub struct End { pub usd: bool, pub rate: BigDec }

/// A candidate: the asset's price in dollars, and the pairs it walked (by id), for ordering and tests.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate { pub price: BigDec, pub legs: Vec<i64> }

/// The venue's priced pairs as steps from each asset: (pair id, next asset, factor). Built once per sync and walked for
/// every holding.
pub struct Graph { steps: BTreeMap<i64, Vec<(i64, i64, BigDec)>> }

impl Graph {
    pub fn new(pairs: &[Pair], quotes: &HashMap<String, BigDec>) -> Graph {
        let mut steps: BTreeMap<i64, Vec<(i64, i64, BigDec)>> = BTreeMap::new();
        for p in pairs {
            let Some(price) = quotes.get(&p.code).filter(|q| q.is_positive()) else { continue };
            if p.base == p.quote { continue; }
            steps.entry(p.base).or_default().push((p.id, p.quote, price.clone()));
            if let Some(inverse) = BigDec::one().div(price) { steps.entry(p.quote).or_default().push((p.id, p.base, inverse)); }
        }
        for list in steps.values_mut() { list.sort_by_key(|(id, _, _)| *id); }
        Graph { steps }
    }

    /// Every candidate of exactly `legs` pairs from `from` to a fiat end, best first (a dollar end before another fiat,
    /// then the pairs' ids), at most `MAX_CANDIDATES`. A route stops at the first fiat it reaches.
    pub fn candidates(&self, ends: &HashMap<i64, End>, from: i64, legs: usize) -> Vec<Candidate> {
        if legs == 0 || legs > MAX_LEGS || ends.contains_key(&from) { return vec![]; }
        let mut found: Vec<(bool, Vec<i64>, BigDec)> = vec![];
        let mut frontier: Vec<(i64, Vec<i64>, Vec<i64>, BigDec)> = vec![(from, vec![from], vec![], BigDec::one())];
        let mut expanded = 0usize;
        for depth in 1..=legs {
            let mut next = vec![];
            for (at, seen, walked, factor) in frontier {
                for (pair, to, step) in self.steps.get(&at).map(Vec::as_slice).unwrap_or(&[]) {
                    expanded += 1;
                    if expanded > MAX_EXPANSIONS { break; }
                    if seen.contains(to) { continue; }
                    let product = &factor * step;
                    let mut path = walked.clone();
                    path.push(*pair);
                    match ends.get(to) {
                        Some(end) => if depth == legs { found.push((!end.usd, path, &product * &end.rate)); },
                        None => if depth < legs { let mut s = seen.clone(); s.push(*to); next.push((*to, s, path, product)); },
                    }
                }
            }
            frontier = next;
        }
        found.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        found.into_iter().take(MAX_CANDIDATES).map(|(_, legs, price)| Candidate { price, legs }).collect()
    }
}

/// `Graph::candidates` over a graph built for the one call.
pub fn candidates(pairs: &[Pair], quotes: &HashMap<String, BigDec>, ends: &HashMap<i64, End>, from: i64, legs: usize) -> Vec<Candidate> {
    Graph::new(pairs, quotes).candidates(ends, from, legs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> BigDec { BigDec::parse(s).unwrap() }
    fn pair(id: i64, code: &str, base: i64, quote: i64) -> Pair { Pair { id, code: code.into(), base, quote } }
    fn quotes(list: &[(&str, &str)]) -> HashMap<String, BigDec> { list.iter().map(|(c, p)| (c.to_string(), d(p))).collect() }
    // Assets: 1 BTC, 2 USD, 3 EUR, 4 USDT, 5 TOKEN, 6 TRY (a fiat with no rate: no end).
    fn ends() -> HashMap<i64, End> { HashMap::from([(2, End { usd: true, rate: BigDec::one() }), (3, End { usd: false, rate: d("1.1") })]) }
    fn best(pairs: &[Pair], q: &HashMap<String, BigDec>, from: i64) -> Option<(String, Vec<i64>)> {
        (1..=MAX_LEGS).find_map(|n| candidates(pairs, q, &ends(), from, n).into_iter().next()).map(|c| (c.price.round(8).to_s_f(), c.legs))
    }

    #[test]
    fn a_direct_dollar_pair_prices_the_asset() {
        assert_eq!(best(&[pair(1, "BTC/USD", 1, 2)], &quotes(&[("BTC/USD", "60000")]), 1), Some(("60000.0".into(), vec![1])));
    }

    #[test]
    fn a_stablecoin_is_routed_never_taken_at_par() {
        // BTCUSDT and EURUSDT only: USDT is priced in euros through the inverse of EURUSDT, then the ECB's euro.
        let pairs = [pair(1, "BTCUSDT", 1, 4), pair(2, "EURUSDT", 3, 4)];
        let q = quotes(&[("BTCUSDT", "55000"), ("EURUSDT", "1.1")]);
        assert_eq!(best(&pairs, &q, 4), Some(("1.0".into(), vec![2])), "USDT = 1/1.1 EUR × 1.1");
        assert_eq!(best(&pairs, &q, 1), Some(("55000.0".into(), vec![1, 2])));
        assert_eq!(best(&[pair(1, "BTCUSDT", 1, 4)], &quotes(&[("BTCUSDT", "55000")]), 1), None, "no fiat in reach");
    }

    #[test]
    fn a_token_quoted_only_in_coins_crosses_through_them() {
        let pairs = [pair(1, "TOKEN/BTC", 5, 1), pair(2, "BTC/USD", 1, 2)];
        assert_eq!(best(&pairs, &quotes(&[("TOKEN/BTC", "0.0001"), ("BTC/USD", "60000")]), 5), Some(("6.0".into(), vec![1, 2])));
    }

    #[test]
    fn a_dollar_end_comes_before_another_fiat_and_shorter_routes_first() {
        let pairs = [pair(1, "BTC/EUR", 1, 3), pair(2, "BTC/USD", 1, 2)];
        let q = quotes(&[("BTC/EUR", "50000"), ("BTC/USD", "60000")]);
        assert_eq!(candidates(&pairs, &q, &ends(), 1, 1).iter().map(|c| c.legs.clone()).collect::<Vec<_>>(), vec![vec![2], vec![1]]);
    }

    #[test]
    fn unpriced_pairs_dead_end_fiats_and_the_leg_cap_are_respected() {
        assert_eq!(best(&[pair(1, "BTC/USD", 1, 2)], &quotes(&[("BTC/USD", "0")]), 1), None, "a zero price is no price");
        assert_eq!(best(&[pair(1, "BTC/USD", 1, 2)], &quotes(&[]), 1), None);
        assert_eq!(best(&[pair(1, "BTC/TRY", 1, 6)], &quotes(&[("BTC/TRY", "2000000")]), 1), None, "a fiat with no rate is no end");
        // Four legs: TOKEN → BTC → USDT → ETH(7) → USD.
        let pairs = [pair(1, "TOKEN/BTC", 5, 1), pair(2, "BTC/USDT", 1, 4), pair(3, "ETH/USDT", 7, 4), pair(4, "ETH/USD", 7, 2)];
        let q = quotes(&[("TOKEN/BTC", "1"), ("BTC/USDT", "1"), ("ETH/USDT", "1"), ("ETH/USD", "1")]);
        assert_eq!(best(&pairs, &q, 5), None);
        assert_eq!(best(&pairs, &q, 1).map(|b| b.1), Some(vec![2, 3, 4]));
    }

    #[test]
    fn every_candidate_of_a_length_is_offered_in_order() {
        // Five two-leg routes BTC → X → USD: all offered, so a caller can pass the unusable ones.
        let mut pairs = vec![];
        let mut q = vec![];
        let codes: Vec<String> = (0..5).flat_map(|i| [format!("BTC/X{i}"), format!("X{i}/USD")]).collect();
        for i in 0..5i64 { pairs.push(pair(10 + i, &codes[2 * i as usize], 1, 100 + i)); pairs.push(pair(20 + i, &codes[2 * i as usize + 1], 100 + i, 2)); }
        for c in &codes { q.push((c.as_str(), "2")); }
        let found = candidates(&pairs, &quotes(&q), &ends(), 1, 2);
        assert_eq!(found.len(), 5);
        assert_eq!(found[3].legs, vec![13, 23]);
    }
}
