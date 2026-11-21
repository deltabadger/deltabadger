//! Delisted coins (plan r10 §1.2–§1.4), PR0: one owner-wide map from every leg of the owner's history to a `Label` that
//! is 1:1 with what the leg holds (a coin, a cash currency, a security, or an account's unidentified units). Pure and
//! unwired until PR1: the resolver's holdings come in, links and outflow inheritance settle the unknowns they can, and
//! the allocator prints a label per identity.
//!
//! A `Label` has no `&str` view: PR1's consumers compare labels, never spellings, and reach a raw spelling only through
//! `Spellings::raw`.
use super::resolver::{CashKind, Holding, Side, Why};
use crate::figures::dec::Dec;
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// One identity's printed label and its identity key (`coin:<id>`, `cash:<spelling>`, `security:<id>`,
/// `unknown:<account>:<spelling>`). Ordered by the printed string, ties broken by the identity (two accounts'
/// unidentified `S` print alike and stay two).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct Label { printed: String, key: String }

impl std::fmt::Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.printed) }
}

/// What a label holds, for the predicates that ask (cash at par, quantity formatting, the sidebar's cash filter).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class { Cash(CashKind), Coin, Security, Unknown }

/// A leg of the owner's history: its row and side, the account and venue it was booked on, its spelling, what the
/// resolver says it holds, when, and its signed quantity (in > 0, out < 0).
#[derive(Clone, Debug)]
pub struct LegIn { pub row: i64, pub side: Side, pub account: i64, pub venue: String, pub spelling: String, pub holding: Holding, pub at: DateTime<Utc>, pub amount: Dec }

/// A stored transfer link: the withdrawal's leg and the deposit's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link { pub from: (i64, Side), pub to: (i64, Side) }

/// A current balance: the account, its asset, the asset's symbol and what it holds.
#[derive(Clone, Debug)]
pub struct BalanceIn { pub account: i64, pub asset_id: i64, pub spelling: String, pub holding: Holding }

/// The catalogue's symbol and name of each coin and security, by external id (a merged label's tie-break, a
/// disambiguated label's name).
pub type Names = HashMap<String, (Option<String>, Option<String>)>;

/// The longest label the allocator prints.
pub const MAX_LABEL: usize = 64;

fn key(account: i64, spelling: &str, h: &Holding) -> String {
    match h {
        Holding::Cash(_) => format!("cash:{spelling}"),
        Holding::Coin(id) => format!("coin:{id}"),
        Holding::Security(id) => format!("security:{id}"),
        Holding::Unknown(_) => format!("unknown:{account}:{spelling}"),
    }
}

fn soft(h: &Holding) -> bool { matches!(h, Holding::Unknown(Why::Unclear)) }

/// §1.2: whether a link's two endpoints may carry one quantity. The same identity, or a known endpoint and an unclear
/// one (the known one wins), or two unclear legs of one spelling (each keeps its account's key). Never into or out of a
/// hard refusal (other units, a cut, a conflict), and never between two different known holdings.
pub fn links_compatible(a: &Holding, a_spelling: &str, b: &Holding, b_spelling: &str) -> bool {
    if a.hard() || b.hard() { return false; }
    match (soft(a), soft(b)) {
        (true, true) => a_spelling == b_spelling,
        (true, false) | (false, true) => true,
        (false, false) => a == b && (!matches!(a, Holding::Cash(_)) || a_spelling == b_spelling),
    }
}

/// The owner's map, built once per pass.
#[derive(Clone, Debug, Default)]
pub struct Spellings {
    legs: HashMap<(i64, Side), Label>,
    holdings: HashMap<Label, Holding>,
    raw: HashMap<Label, Vec<String>>,
    assets: HashMap<i64, Label>,
    void: Vec<Link>,
}

/// Keeps the label alphabet: `[A-Za-z0-9 .()-]`, single spaces, trimmed.
fn sanitise(s: &str) -> String {
    let kept: String = s.chars().filter(|c| c.is_ascii_alphanumeric() || " .()-".contains(*c)).collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `<spelling> (<detail>)`, within the alphabet and MAX_LABEL.
fn decorated(spelling: &str, detail: &str) -> String {
    let spelling: String = sanitise(spelling).chars().take(MAX_LABEL / 2).collect();
    let room = MAX_LABEL.saturating_sub(spelling.len() + 3);
    let detail: String = sanitise(detail).chars().take(room).collect::<String>().trim().to_string();
    if spelling.is_empty() { format!("({detail})") } else { format!("{spelling} ({detail})") }
}

struct Identity { holding: Holding, venue: String, uses: BTreeMap<String, usize> }

impl Spellings {
    /// The map for one owner: every leg of the whole history (pending ones too), its stored links, its current
    /// balances, and the catalogue's names.
    pub fn build(mut legs: Vec<LegIn>, links: &[Link], balances: &[BalanceIn], names: &Names) -> Spellings {
        legs.sort_by(|a, b| (a.at, a.row, a.side).cmp(&(b.at, b.row, b.side)));
        let index: HashMap<(i64, Side), usize> = legs.iter().enumerate().map(|(i, l)| ((l.row, l.side), i)).collect();
        let mut eff: Vec<Holding> = legs.iter().map(|l| l.holding.clone()).collect();
        let mut void = vec![];
        let mut valid: HashMap<usize, usize> = HashMap::new(); // source leg → destination leg
        // Links: the known endpoint's holding wins in either direction; an incompatible link is void for identity and
        // quantity.
        for link in links {
            let (Some(&a), Some(&b)) = (index.get(&link.from), index.get(&link.to)) else { continue };
            if !links_compatible(&eff[a], &legs[a].spelling, &eff[b], &legs[b].spelling) { void.push(*link); continue; }
            if soft(&eff[a]) && !soft(&eff[b]) { eff[a] = eff[b].clone(); } else if soft(&eff[b]) && !soft(&eff[a]) { eff[b] = eff[a].clone(); }
            valid.insert(a, b);
        }
        // Outflow inheritance, in order: an unclear outflow takes X when X is the only identity held under its spelling at
        // its account, and covers it. What it then holds crosses its link.
        let mut held: HashMap<(i64, String), BTreeMap<String, (Holding, Dec)>> = HashMap::new();
        for i in 0..legs.len() {
            let l = &legs[i];
            let at = held.entry((l.account, l.spelling.clone())).or_default();
            if soft(&eff[i]) && l.amount.is_negative() {
                let positive: Vec<&(Holding, Dec)> = at.values().filter(|(_, q)| q.is_positive()).collect();
                if let [(h, q)] = positive.as_slice() {
                    if h.known() && (q + &l.amount).is_ok_and(|left| !left.is_negative()) { eff[i] = h.clone(); }
                }
                if let Some(&to) = valid.get(&i) { if soft(&eff[to]) && eff[i].known() { eff[to] = eff[i].clone(); } }
            }
            let entry = at.entry(key(l.account, &l.spelling, &eff[i])).or_insert_with(|| (eff[i].clone(), Dec::zero()));
            if let Ok(sum) = &entry.1 + &l.amount { entry.1 = sum; }
        }
        // The identities and their spellings, balances included (a balance of Y under X's spelling makes it two).
        let mut ids: BTreeMap<String, Identity> = BTreeMap::new();
        let mut leg_keys = vec![];
        for (l, h) in legs.iter().zip(&eff) {
            let k = key(l.account, &l.spelling, h);
            let id = ids.entry(k.clone()).or_insert_with(|| Identity { holding: h.clone(), venue: l.venue.clone(), uses: BTreeMap::new() });
            *id.uses.entry(l.spelling.clone()).or_default() += 1;
            leg_keys.push(((l.row, l.side), k));
        }
        let mut asset_keys = vec![];
        for b in balances {
            let k = key(b.account, &b.spelling, &b.holding);
            ids.entry(k.clone()).or_insert_with(|| Identity { holding: b.holding.clone(), venue: String::new(), uses: BTreeMap::new() })
                .uses.entry(b.spelling.clone()).or_default();
            asset_keys.push((b.asset_id, k));
        }
        let labels = allocate(&ids, names);
        let label = |k: &String| labels[k].clone();
        let mut s = Spellings { void, ..Spellings::default() };
        for (k, id) in &ids {
            s.holdings.insert(label(k), id.holding.clone());
            s.raw.insert(label(k), id.uses.keys().cloned().collect());
        }
        s.legs = leg_keys.into_iter().map(|(leg, k)| (leg, label(&k))).collect();
        s.assets = asset_keys.into_iter().map(|(a, k)| (a, label(&k))).collect();
        s
    }

    /// The label of a leg (a row's base, quote or fee).
    pub fn label(&self, row: i64, side: Side) -> Option<&Label> { self.legs.get(&(row, side)) }
    /// What a label holds.
    pub fn holding(&self, label: &Label) -> Option<&Holding> { self.holdings.get(label) }
    /// The owner's raw spellings behind a label, sorted: candidates for SQL that gathers by spelling (each row is then
    /// filtered by its own leg's label).
    pub fn raw(&self, label: &Label) -> &[String] { self.raw.get(label).map_or(&[], Vec::as_slice) }
    /// The label of a current balance's asset.
    pub fn label_of_asset(&self, asset_id: i64) -> Option<&Label> { self.assets.get(&asset_id) }
    pub fn class(&self, label: &Label) -> Class {
        match self.holdings.get(label) {
            Some(Holding::Cash(k)) => Class::Cash(*k),
            Some(Holding::Coin(_)) => Class::Coin,
            Some(Holding::Security(_)) => Class::Security,
            _ => Class::Unknown,
        }
    }
    /// The links void for identity and quantity (`transfer_conflict`).
    pub fn void_links(&self) -> &[Link] { &self.void }
}

/// §1.4: one label per identity, deterministic. Cash and securities print their spelling. A coin prints its spelling
/// when it has one that no other identity uses and that is no cash spelling; with several, the most used (tie: the
/// catalogue symbol, then the first); a reserved one is printed `<spelling> (<name>)`, then with its external id, then
/// with a counter, never equal to another label or to any raw spelling. An unidentified unit prints its spelling unless a
/// known identity uses it, then `<spelling> (unidentified on <venue>)`.
fn allocate(ids: &BTreeMap<String, Identity>, names: &Names) -> HashMap<String, Label> {
    let mut users: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    for (k, id) in ids { for s in id.uses.keys() { users.entry(s.as_str()).or_default().insert(k.as_str()); } }
    let raw: BTreeSet<&str> = users.keys().copied().collect();
    let mut taken: BTreeSet<String> = BTreeSet::new();
    let mut out = HashMap::new();
    let first = |id: &Identity| id.uses.keys().next().cloned().unwrap_or_default();
    let rank = |h: &Holding| match h { Holding::Cash(_) | Holding::Security(_) => 0, Holding::Coin(_) => 1, Holding::Unknown(_) => 2 };
    let mut order: Vec<(&String, &Identity)> = ids.iter().collect();
    order.sort_by_key(|(k, id)| (rank(&id.holding), (*k).clone()));
    for (k, id) in order {
        let printed = match &id.holding {
            Holding::Cash(_) | Holding::Security(_) => id.uses.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map(|(s, _)| s.clone()).unwrap_or_default(),
            Holding::Coin(ext) => {
                let (symbol, name) = names.get(ext).cloned().unwrap_or_default();
                let best = id.uses.iter().max_by(|a, b| a.1.cmp(b.1).then((Some(a.0) == symbol.as_ref()).cmp(&(Some(b.0) == symbol.as_ref()))).then(b.0.cmp(a.0)))
                    .map(|(s, _)| s.clone()).unwrap_or_default();
                let reserved = super::cash(&best) || users.get(best.as_str()).is_some_and(|u| u.len() > 1) || taken.contains(&best);
                if !reserved { best } else {
                    let base = decorated(&best, name.as_deref().filter(|n| !sanitise(n).is_empty()).unwrap_or(ext));
                    let free = |s: &String| !taken.contains(s) && !raw.contains(s.as_str());
                    let with_id = decorated(&best, &format!("{} {ext}", name.as_deref().unwrap_or(ext)));
                    [base, with_id.clone()].into_iter().find(free).unwrap_or_else(|| (2..).map(|n| format!("{} {n}", with_id.chars().take(MAX_LABEL - 6).collect::<String>())).find(free).unwrap())
                }
            }
            Holding::Unknown(_) => {
                let spelling = first(id);
                let known = users.get(spelling.as_str()).is_some_and(|u| u.iter().any(|o| !o.starts_with("unknown:")));
                if known { decorated(&spelling, &format!("unidentified on {}", id.venue)) } else { spelling }
            }
        };
        if !matches!(id.holding, Holding::Unknown(_)) { taken.insert(printed.clone()); }
        out.insert(k.clone(), Label { printed, key: k.clone() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: &str) -> DateTime<Utc> { format!("{day}T12:00:00Z").parse().unwrap() }
    fn d(s: &str) -> Dec { Dec::strict(s).unwrap() }
    fn coin(ext: &str) -> Holding { Holding::Coin(ext.into()) }
    const UNCLEAR: Holding = Holding::Unknown(Why::Unclear);
    #[allow(clippy::too_many_arguments)] // a leg's fields
    fn leg(row: i64, side: Side, account: i64, venue: &str, spelling: &str, holding: Holding, day: &str, amount: &str) -> LegIn {
        LegIn { row, side, account, venue: venue.into(), spelling: spelling.into(), holding, at: at(day), amount: d(amount) }
    }
    fn base(row: i64, account: i64, spelling: &str, holding: Holding, day: &str, amount: &str) -> LegIn { leg(row, Side::Base, account, "Binance", spelling, holding, day, amount) }
    fn names(list: &[(&str, &str, &str)]) -> Names { list.iter().map(|(e, s, n)| (e.to_string(), (Some(s.to_string()), Some(n.to_string())))).collect() }
    fn printed(s: &Spellings, row: i64) -> String { s.label(row, Side::Base).unwrap().to_string() }
    fn alphabet(label: &str) -> bool { label.len() <= MAX_LABEL && label.chars().all(|c| c.is_ascii_alphanumeric() || " .()-".contains(c)) }

    /// §1.4: a spelling 1:1 with one coin prints as itself (the byte-identical common case); a spelling two coins share
    /// prints each by its name; a coin under two spellings prints the most used, unless another coin also uses it.
    #[test]
    fn labels_one_coin_one_spelling_and_disambiguate_the_rest() {
        let n = names(&[("bitcoin", "BTC", "Bitcoin"), ("bitcoin-gold", "BTG", "Bitcoin Gold"), ("openverse-network", "BTG", "Openverse Network"),
                        ("terra-luna", "LUNA", "Terra Luna Classic"), ("terra-luna-2", "LUNA", "Terra")]);
        let legs = vec![base(1, 1, "BTC", coin("bitcoin"), "2020-01-01", "1"), base(2, 1, "BTG", coin("bitcoin-gold"), "2018-01-01", "2"),
                        base(3, 2, "BTG", coin("openverse-network"), "2025-01-01", "5"),
                        // terra-luna under LUNA (before the rename) and LUNC (more often); terra-luna-2 under LUNA.
                        base(4, 1, "LUNA", coin("terra-luna"), "2021-01-01", "1"), base(5, 1, "LUNC", coin("terra-luna"), "2023-01-01", "1"),
                        base(6, 1, "LUNC", coin("terra-luna"), "2023-02-01", "1"), base(7, 1, "LUNA", coin("terra-luna-2"), "2023-03-01", "1")];
        let s = Spellings::build(legs, &[], &[], &n);
        assert_eq!(printed(&s, 1), "BTC");
        assert_eq!((printed(&s, 2), printed(&s, 3)), ("BTG (Bitcoin Gold)".into(), "BTG (Openverse Network)".into()));
        assert_eq!((printed(&s, 4), printed(&s, 5)), ("LUNC".into(), "LUNC".into()), "one coin, one label: the most used spelling");
        assert_eq!(printed(&s, 7), "LUNA (Terra)", "LUNA is another coin's spelling too");
        assert_eq!(s.raw(s.label(4, Side::Base).unwrap()), ["LUNA", "LUNC"]);
        assert_eq!(s.holding(s.label(2, Side::Base).unwrap()), Some(&coin("bitcoin-gold")));
        assert_eq!(s.class(s.label(1, Side::Base).unwrap()), Class::Coin);
    }

    /// §1.4's property cases: two coins with one name; a real spelling equal to a generated label; the złoty and PLEARN
    /// (cash spellings are reserved); a security and a coin of one spelling. Every known identity's label is unique,
    /// within the alphabet, and never another identity's raw spelling.
    #[test]
    fn labels_stay_unique_and_in_the_alphabet() {
        let n = names(&[("x-one", "TWIN", "Same Name"), ("x-two", "TWIN", "Same Name"), ("y-coin", "Y", "Y Coin"), ("z-coin", "Y", "Zed"),
                        ("pleasure-coin", "PLN", "PLEARN"), ("compound", "COMP", "Compound, the coin"), ("weird", "W", "Wé, ird/Coin \"x\"")]);
        let legs = vec![base(1, 1, "TWIN", coin("x-one"), "2024-01-01", "1"), base(2, 1, "TWIN", coin("x-two"), "2024-01-02", "1"),
                        base(3, 1, "Y", coin("y-coin"), "2024-01-03", "1"), base(4, 1, "Y", coin("z-coin"), "2024-01-04", "1"),
                        // A real spelling that equals what the allocator would print first for y-coin.
                        base(5, 1, "Y (Y Coin)", coin("other"), "2024-01-05", "1"),
                        base(6, 3, "PLN", Holding::Cash(CashKind::Fiat), "2024-01-06", "100"), base(7, 4, "PLN", coin("pleasure-coin"), "2024-01-07", "1"),
                        base(8, 5, "COMP", Holding::Security("comp-stock".into()), "2024-01-08", "1"), base(9, 1, "COMP", coin("compound"), "2024-01-09", "1"),
                        base(10, 1, "W", coin("weird"), "2024-01-10", "1"), base(11, 1, "W", coin("weird-2"), "2024-01-11", "1")];
        let s = Spellings::build(legs, &[], &[], &n);
        let all: Vec<String> = (1..=11).map(|r| printed(&s, r)).collect();
        assert_eq!(&all[0..2], ["TWIN (Same Name)", "TWIN (Same Name x-two)"], "the second same name takes its external id");
        assert_eq!(printed(&s, 3), "Y (Y Coin y-coin)", "a real spelling holds the first form");
        assert_eq!(printed(&s, 5), "Y (Y Coin)");
        assert_eq!((printed(&s, 6), printed(&s, 7)), ("PLN".into(), "PLN (PLEARN)".into()));
        assert_eq!((printed(&s, 8), printed(&s, 9)), ("COMP".into(), "COMP (Compound the coin)".into()));
        assert_eq!(printed(&s, 10), "W (W irdCoin x)", "sanitised: no comma, quote or slash");
        let unique: BTreeSet<&String> = all.iter().collect();
        assert_eq!(unique.len(), all.len(), "{all:?}");
        let raws = ["TWIN", "Y", "Y (Y Coin)", "PLN", "COMP", "W"];
        for (r, label) in all.iter().enumerate() {
            assert!(alphabet(label), "{label}");
            let own = s.raw(s.label(r as i64 + 1, Side::Base).unwrap());
            assert!(!raws.contains(&label.as_str()) || own.contains(label), "{label} is another identity's spelling");
        }
        let long = Spellings::build(vec![base(1, 1, "S", coin("a"), "2024-01-01", "1"), base(2, 1, "S", coin("b"), "2024-01-01", "1")], &[], &[],
                                    &names(&[("a", "S", &"N".repeat(200))]));
        assert!(alphabet(&printed(&long, 1)) && printed(&long, 1).ends_with(')'), "{}", printed(&long, 1));
    }

    /// Codex P1-2 (r10): labels order by their printed string, then by identity: two accounts' unidentified `S` print
    /// alike and stay two keys; a known coin under `S` marks both as unidentified. A label survives a cache round trip.
    #[test]
    fn label_ord_breaks_ties_by_identity() {
        let legs = vec![base(1, 1, "S", UNCLEAR, "2024-01-01", "1"), base(2, 2, "S", UNCLEAR, "2024-01-02", "1")];
        let s = Spellings::build(legs.clone(), &[], &[], &Names::new());
        let (a, b) = (s.label(1, Side::Base).unwrap().clone(), s.label(2, Side::Base).unwrap().clone());
        assert_eq!((a.to_string(), b.to_string()), ("S".into(), "S".into()));
        assert_ne!(a, b);
        assert_eq!(a.cmp(&b), std::cmp::Ordering::Less, "the identity breaks the tie");
        assert_eq!(s.class(&a), Class::Unknown);
        let mut more = legs;
        more.push(base(3, 1, "S", coin("s-coin"), "2024-01-03", "1"));
        let s = Spellings::build(more, &[], &[], &Names::new());
        assert_eq!((printed(&s, 1), printed(&s, 3)), ("S (unidentified on Binance)".into(), "S (s-coin)".into()));
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(serde_json::from_str::<Label>(&json).unwrap(), a);
    }

    /// §1.3: an unidentified unit's key is its account's: an unclear buy on one account and an unclear sale on another
    /// are two identities; on one account, one.
    #[test]
    fn the_unknown_key_is_per_account() {
        let s = Spellings::build(vec![base(1, 1, "S", UNCLEAR, "2024-01-01", "1"), base(2, 2, "S", UNCLEAR, "2024-02-01", "-1"),
                                      base(3, 1, "S", UNCLEAR, "2024-03-01", "-1")], &[], &[], &Names::new());
        assert_ne!(s.label(1, Side::Base), s.label(2, Side::Base));
        assert_eq!(s.label(1, Side::Base), s.label(3, Side::Base));
    }

    /// §1.2 links: the known endpoint wins in either direction; two unclear legs of one spelling link across accounts;
    /// never into or out of a hard refusal (a cut, `1000REKT`'s other units, a conflict), never two different coins.
    #[test]
    fn manual_link_same_spelling_unknowns_across_accounts() {
        let cut = Holding::Unknown(Why::Cut);
        let units = Holding::Unknown(Why::Units);
        assert!(links_compatible(&UNCLEAR, "S", &UNCLEAR, "S"));
        assert!(!links_compatible(&UNCLEAR, "S", &UNCLEAR, "T"));
        assert!(links_compatible(&UNCLEAR, "S", &coin("x"), "S") && links_compatible(&coin("x"), "X", &UNCLEAR, "S"));
        assert!(links_compatible(&coin("x"), "X", &coin("x"), "XX"), "one coin under two spellings");
        assert!(!links_compatible(&coin("x"), "S", &coin("y"), "S"));
        assert!(!links_compatible(&cut, "PHB", &coin("x"), "PHB") && !links_compatible(&UNCLEAR, "PHB", &cut, "PHB"));
        assert!(!links_compatible(&units, "1000REKT", &coin("rekt"), "REKT"));
        assert!(!links_compatible(&Holding::Unknown(Why::Conflict), "S", &coin("x"), "S"));
        assert!(!links_compatible(&Holding::Cash(CashKind::Fiat), "EUR", &Holding::Cash(CashKind::Fiat), "USD"));
        // In the map: a known withdrawal makes its unclear deposit that coin; a void link is reported and carries nothing.
        let legs = vec![base(1, 1, "S", coin("x"), "2024-01-01", "1"), base(2, 1, "S", coin("x"), "2024-01-02", "-1"),
                        base(3, 2, "S", UNCLEAR, "2024-01-02", "1"),
                        base(4, 1, "1000REKT", units.clone(), "2024-01-03", "-1"), base(5, 2, "REKT", coin("rekt"), "2024-01-03", "1")];
        let links = [Link { from: (2, Side::Base), to: (3, Side::Base) }, Link { from: (4, Side::Base), to: (5, Side::Base) }];
        let s = Spellings::build(legs, &links, &[], &Names::new());
        assert_eq!(s.label(3, Side::Base), s.label(1, Side::Base), "the deposit took the withdrawal's coin");
        assert_eq!(s.void_links(), &links[1..]);
        assert_eq!(s.holding(s.label(4, Side::Base).unwrap()), Some(&units));
    }

    /// §1.2 inheritance: an unclear outflow takes X only when X is the only identity held under its spelling at its
    /// account and covers it; X and Y both held, nothing held, or only unidentified units held leave it unknown; a hard
    /// refusal never inherits; an inflow never does; what it inherits crosses its link.
    #[test]
    fn inheritance_needs_exactly_one_source() {
        let s = Spellings::build(vec![base(1, 1, "S", coin("x"), "2024-01-01", "2"), base(2, 1, "S", UNCLEAR, "2024-01-02", "-1"),
                                      base(3, 1, "S", UNCLEAR, "2024-01-03", "-5"), base(4, 1, "S", UNCLEAR, "2024-01-04", "1")], &[], &[], &Names::new());
        assert_eq!(s.holding(s.label(2, Side::Base).unwrap()), Some(&coin("x")), "X alone covers it");
        assert_eq!(s.class(s.label(3, Side::Base).unwrap()), Class::Unknown, "more than X holds");
        assert_eq!(s.class(s.label(4, Side::Base).unwrap()), Class::Unknown, "an inflow");
        let both = Spellings::build(vec![base(1, 1, "S", coin("x"), "2024-01-01", "2"), base(2, 1, "S", coin("y"), "2024-01-02", "2"),
                                         base(3, 1, "S", UNCLEAR, "2024-01-03", "-1")], &[], &[], &Names::new());
        assert_eq!(both.class(both.label(3, Side::Base).unwrap()), Class::Unknown, "X and Y both held");
        let gone = Spellings::build(vec![base(1, 1, "S", coin("x"), "2024-01-01", "1"), base(2, 1, "S", coin("x"), "2024-01-02", "-1"),
                                         base(3, 1, "S", UNCLEAR, "2024-01-03", "1"), base(4, 1, "S", UNCLEAR, "2024-01-04", "-1")], &[], &[], &Names::new());
        assert_eq!(gone.class(gone.label(4, Side::Base).unwrap()), Class::Unknown, "only unidentified units are held");
        let other = Spellings::build(vec![base(1, 1, "S", coin("x"), "2024-01-01", "2"), base(2, 2, "S", UNCLEAR, "2024-01-02", "-1")], &[], &[], &Names::new());
        assert_eq!(other.class(other.label(2, Side::Base).unwrap()), Class::Unknown, "X is held at another account");
        let cut = Spellings::build(vec![base(1, 1, "PHB", coin("x"), "2024-01-01", "2"), base(2, 1, "PHB", Holding::Unknown(Why::Cut), "2024-01-02", "-1")],
                                   &[], &[], &Names::new());
        assert_eq!(cut.holding(cut.label(2, Side::Base).unwrap()), Some(&Holding::Unknown(Why::Cut)), "a hard refusal never inherits");
        let linked = Spellings::build(vec![base(1, 1, "S", coin("x"), "2024-01-01", "2"), base(2, 1, "S", UNCLEAR, "2024-01-02", "-1"),
                                           base(3, 2, "S", UNCLEAR, "2024-01-02", "1")], &[Link { from: (2, Side::Base), to: (3, Side::Base) }], &[], &Names::new());
        assert_eq!(linked.holding(linked.label(3, Side::Base).unwrap()), Some(&coin("x")), "the inherited coin crosses the link");
    }

    /// Balances are an input: a balance of Y under the spelling the history uses for X makes the spelling two coins, and
    /// the balance's asset finds its label.
    #[test]
    fn a_balance_under_a_historys_spelling_splits_it() {
        let balances = [BalanceIn { account: 1, asset_id: 77, spelling: "S".into(), holding: coin("y") }];
        let s = Spellings::build(vec![base(1, 1, "S", coin("x"), "2024-01-01", "1")], &[], &balances, &names(&[("x", "S", "Ex"), ("y", "S", "Why")]));
        assert_eq!(printed(&s, 1), "S (Ex)");
        assert_eq!(s.label_of_asset(77).unwrap().to_string(), "S (Why)");
        assert_eq!(s.label_of_asset(78), None);
    }
}
