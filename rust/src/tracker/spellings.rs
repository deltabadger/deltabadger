//! Delisted coins (plan r10 §1.2–§1.4), PR0: one owner-wide map from every leg of the owner's history to a `Label` that
//! is 1:1 with what the leg holds (a coin, a cash currency, a security, or an account's unidentified units). Pure and
//! unwired until PR1: the resolver's holdings come in, links and outflow inheritance settle the unknowns they can, and
//! the allocator prints a label per identity.
//!
//! A `Label` has no `&str` view: PR1's consumers compare labels, never spellings, and reach a raw spelling only through
//! `Spellings::raw`.
use super::resolver::{other_units, CashKind, Holding, Side, Why};
use crate::figures::dec::Dec;
use crate::figures::num::NumError;
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// One identity's printed label and its identity key (`coin:<id>`, `cash:<spelling>`, `security:<id>`,
/// `unknown:<account>:<spelling>`). Ordered by the printed string, ties broken by the identity (two accounts'
/// unidentified `S` print alike and stay two).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct Label { printed: String, key: String }

/// PR1 step 1, identity mode: `Stored` and `walk::Row` carry a `Label` that is their row's own spelling (an empty
/// identity key, so a spelled label allocates nothing beyond its text), and every output stays byte-identical. The
/// spelling view below (`Deref<Target = str>`, equality with strings, `ToSql`/`FromSql`, the `From` impls) exists only for
/// spelled labels: string equality is not transitive across two allocated labels that print alike. Step 2 builds the
/// labels from `Spellings` and deletes that view, so the compiler names every reader still reading a label as a spelling.
impl Label {
    pub fn spelled(s: impl Into<String>) -> Label { Label { printed: s.into(), key: String::new() } }
    pub fn as_str(&self) -> &str { &self.printed }
}
impl From<&str> for Label { fn from(s: &str) -> Label { Label::spelled(s) } }
impl From<String> for Label { fn from(s: String) -> Label { Label::spelled(s) } }
impl From<&String> for Label { fn from(s: &String) -> Label { Label::spelled(s.as_str()) } }
impl From<Label> for String { fn from(l: Label) -> String { l.printed } }
impl rusqlite::types::ToSql for Label {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> { self.printed.to_sql() }
}
impl rusqlite::types::FromSql for Label {
    fn column_result(v: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Label> { String::column_result(v).map(Label::spelled) }
}
impl Default for Label { fn default() -> Label { Label::spelled("") } }
impl std::ops::Deref for Label { type Target = str; fn deref(&self) -> &str { &self.printed } }
impl PartialEq<str> for Label { fn eq(&self, o: &str) -> bool { self.printed == o } }
impl PartialEq<&str> for Label { fn eq(&self, o: &&str) -> bool { self.printed == *o } }
impl PartialEq<String> for Label { fn eq(&self, o: &String) -> bool { &self.printed == o } }
impl PartialEq<Label> for String { fn eq(&self, o: &Label) -> bool { *self == o.printed } }
impl PartialEq<Label> for &str { fn eq(&self, o: &Label) -> bool { *self == o.printed } }

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Link { pub from: (i64, Side), pub to: (i64, Side) }

/// A current balance: the account and its venue, its asset, the asset's symbol and what it holds.
#[derive(Clone, Debug)]
pub struct BalanceIn { pub account: i64, pub venue: String, pub asset_id: i64, pub spelling: String, pub holding: Holding }

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
const UNCLEAR_HOLDING: Holding = Holding::Unknown(Why::Unclear);

/// What each account holds, as a pass books its legs: the quantity of each lot key, the keys each (account, spelling)
/// has carried, each key's first holding, and the reason the units outstanding under a lot carry (a spent unclear lot
/// refilled by a conflicting deposit is a conflicting lot).
#[derive(Default)]
struct Books {
    held: HashMap<(i64, String), Dec>,
    carried: HashMap<(i64, String), BTreeSet<String>>,
    of_key: HashMap<String, Holding>,
    lot: HashMap<(i64, String), Holding>,
}

impl Books {
    fn book(&mut self, account: i64, spelling: &str, k: &str, h: &Holding, amount: &Dec) -> Result<(), NumError> {
        self.of_key.entry(k.to_string()).or_insert_with(|| h.clone());
        let mine = if k == key(account, spelling, h) { h.clone() } else { self.of_key[k].clone() };
        let q = self.held.entry((account, k.to_string())).or_insert_with(Dec::zero);
        let empty = !q.is_positive();
        *q = (&*q + amount)?;
        let positive = q.is_positive();
        let lk = (account, k.to_string());
        if !positive { self.lot.remove(&lk); }
        else if empty || !self.lot.contains_key(&lk) { self.lot.insert(lk, mine); }
        else if amount.is_positive() {
            if let (Some(Holding::Unknown(a)), Holding::Unknown(b)) = (self.lot.get_mut(&lk), &mine) { *a = (*a).min(*b); }
        }
        self.carried.entry((account, spelling.to_string())).or_default().insert(k.to_string());
        Ok(())
    }
}

/// One instant's legs (already in ledger order) in an order the transfers and the accounts allow: a transfer's source
/// before its destination, each account's legs in ledger order, otherwise ledger order. Where those disagree (a cycle),
/// the rest keep ledger order.
fn ordered(legs: Vec<LegIn>, links: &[Link]) -> Vec<LegIn> {
    let mut out = Vec::with_capacity(legs.len());
    let mut rest = legs.into_iter().peekable();
    while let Some(first) = rest.next() {
        let mut group = vec![first];
        while rest.peek().is_some_and(|l| l.at == group[0].at) { group.extend(rest.next()); }
        if group.len() < 2 { out.extend(group); continue; }
        let at: HashMap<(i64, Side), usize> = group.iter().enumerate().map(|(i, l)| ((l.row, l.side), i)).collect();
        let mut after: Vec<Vec<usize>> = vec![vec![]; group.len()];
        let mut needs = vec![0usize; group.len()];
        let mut edge = |a: usize, b: usize, after: &mut Vec<Vec<usize>>| { after[a].push(b); needs[b] += 1; };
        for link in links { if let (Some(&a), Some(&b)) = (at.get(&link.from), at.get(&link.to)) { edge(a, b, &mut after); } }
        let mut last: HashMap<i64, usize> = HashMap::new();
        for (i, l) in group.iter().enumerate() { if let Some(p) = last.insert(l.account, i) { edge(p, i, &mut after); } }
        let mut ready: BTreeSet<usize> = (0..group.len()).filter(|&i| needs[i] == 0).collect();
        let mut order = vec![];
        while let Some(i) = ready.pop_first() {
            order.push(i);
            for &j in &after[i] { needs[j] -= 1; if needs[j] == 0 { ready.insert(j); } }
        }
        let placed: BTreeSet<usize> = order.iter().copied().collect();
        order.extend((0..group.len()).filter(|i| !placed.contains(i)));
        let mut slots: Vec<Option<LegIn>> = group.into_iter().map(Some).collect();
        out.extend(order.into_iter().filter_map(|i| slots[i].take()));
    }
    out
}

/// §1.2: whether a link's two endpoints may carry one quantity: the same holding, or a known endpoint and an unclear one
/// (the known one wins) whose spelling is no other unit of the known one's spelling or catalogue symbol (Kraken's XBT is
/// Bitcoin: `1000BTC` is another unit of it), or two unclear legs of one spelling (each keeps its account's key). Cash is
/// its spelling, so it links only with the same cash. Never into or out of a hard refusal (other units, a cut, a
/// conflict), and never between two different known holdings.
pub fn links_compatible(a: &Holding, a_spelling: &str, b: &Holding, b_spelling: &str, names: &Names) -> bool {
    if a.hard() || b.hard() { return false; }
    if matches!(a, Holding::Cash(_)) || matches!(b, Holding::Cash(_)) { return a == b && a_spelling == b_spelling; }
    let units = |unclear: &str, known: &Holding, spelled: &str| {
        let symbol = match known { Holding::Coin(e) | Holding::Security(e) => names.get(e).and_then(|n| n.0.clone()), _ => None };
        std::iter::once(spelled.to_string()).chain(symbol).any(|k| other_units(unclear, &k) || other_units(&k, unclear))
    };
    match (soft(a), soft(b)) {
        (true, true) => a_spelling == b_spelling,
        (true, false) => !units(a_spelling, b, b_spelling),
        (false, true) => !units(b_spelling, a, a_spelling),
        (false, false) => a == b,
    }
}

/// One walk of the books.
struct Walked { legs: Vec<LegIn>, eff: Vec<Holding>, keys: Vec<String>, sources: HashMap<usize, (usize, Link)> }

/// The books walked with `live` as the transfers: `Err(link)` at the first link that cannot be one (its ends
/// incompatible; no single lot to move, another coin's lot, too little).
fn walk(legs: &[LegIn], live: &[Link], names: &Names) -> Result<Result<Walked, Link>, NumError> {
    let legs = ordered(legs.to_vec(), live);
    let index: HashMap<(i64, Side), usize> = legs.iter().enumerate().map(|(i, l)| ((l.row, l.side), i)).collect();
    let mut eff: Vec<Holding> = legs.iter().map(|l| l.holding.clone()).collect();
    let mut sources: HashMap<usize, (usize, Link)> = HashMap::new();
    for link in live {
        let (Some(&a), Some(&b)) = (index.get(&link.from), index.get(&link.to)) else { continue };
        if !links_compatible(&eff[a], &legs[a].spelling, &eff[b], &legs[b].spelling, names) { return Ok(Err(*link)); }
        // A known source gives the deposit its holding now; an unclear source is settled at its instant (below).
        if !soft(&eff[a]) && soft(&eff[b]) { eff[b] = eff[a].clone(); }
        sources.insert(a, (b, *link));
    }
    // In order: what each account holds of each identity (`Books`). A linked deposit lands with its withdrawal,
    // as the tracker books a transfer (backfill.rs `withdraw`): it is booked right after the withdrawal, and
    // skipped when its own turn comes (an equal-time cycle cannot book it before the coin it carries is known).
    let mut b = Books::default();
    let pairs: HashMap<usize, usize> = sources.iter().map(|(a, (d, _))| (*a, *d)).collect();
    let arrivals: HashMap<usize, usize> = pairs.iter().map(|(a, d)| (*d, *a)).collect();
    let mut keys: Vec<Option<String>> = vec![None; legs.len()];
    for i in 0..legs.len() {
        if keys[i].is_some() || arrivals.get(&i).is_some_and(|src| keys[*src].is_none()) { continue; }
        let l = &legs[i];
        let mut debit = None;
        if soft(&eff[i]) && l.amount.is_negative() {
            let positive: Vec<String> = b.carried.get(&(l.account, l.spelling.clone())).into_iter().flatten()
                .filter(|k| b.held.get(&(l.account, (*k).clone())).is_some_and(Dec::is_positive)).cloned().collect();
            let reason = |k: &String| b.lot.get(&(l.account, k.clone())).or_else(|| b.of_key.get(k)).cloned().unwrap_or(UNCLEAR_HOLDING);
            match (sources.get(&i).copied(), positive.as_slice()) {
                // A transfer out of an unclear leg moves the one lot its account holds under the spelling, when
                // that lot is compatible with the deposit and covers the amount; otherwise (none, several,
                // another coin, a refused lot, too little) the link is void (r10 P1-6).
                (Some((to, link)), [one]) => {
                    let source = reason(one);
                    if !links_compatible(&source, &l.spelling, &eff[to], &legs[to].spelling, names) || (&b.held[&(l.account, one.clone())] + &l.amount)?.is_negative() {
                        return Ok(Err(link));
                    }
                    debit = Some(one.clone());
                    if source.known() { eff[i] = source.clone(); if soft(&eff[to]) { eff[to] = source; } }
                    else if !soft(&eff[to]) { eff[i] = eff[to].clone(); }
                }
                (Some((_, link)), _) => return Ok(Err(link)),
                // Outflow inheritance: X alone held under the spelling, and enough of it.
                (None, [one]) => {
                    let h = reason(one);
                    if h.known() && !(&b.held[&(l.account, one.clone())] + &l.amount)?.is_negative() { eff[i] = h; }
                }
                (None, _) => {}
            }
        }
        let k = debit.unwrap_or_else(|| key(l.account, &l.spelling, &eff[i]));
        b.book(l.account, &l.spelling, &k, &eff[i], &l.amount)?;
        keys[i] = Some(k);
        // The deposit, with its withdrawal.
        if let Some(&to) = pairs.get(&i) {
            if keys[to].is_none() {
                let d = &legs[to];
                let k = key(d.account, &d.spelling, &eff[to]);
                b.book(d.account, &d.spelling, &k, &eff[to], &d.amount)?;
                keys[to] = Some(k);
            }
        }
    }
    Ok(Ok(Walked { legs, eff, keys: keys.into_iter().map(|k| k.expect("every leg is booked")).collect(), sources }))
}

/// The owner's map, built once per pass.
#[derive(Clone, Debug, Default)]
pub struct Spellings {
    /// Each leg: its label (the lot key its quantity is booked under) and its effective holding.
    legs: HashMap<(i64, Side), (Label, Holding)>,
    holdings: HashMap<Label, Holding>,
    raw: HashMap<Label, Vec<String>>,
    assets: HashMap<(i64, i64), Label>,
    /// A valid link's source leg → (the label its quantity leaves, the label it arrives under).
    transfers: HashMap<(i64, Side), (Label, Label)>,
    void: Vec<Link>,
}

/// Keeps the label alphabet: `[A-Za-z0-9 .()-]`, single spaces, trimmed.
fn sanitise(s: &str) -> String {
    let kept: String = s.chars().filter(|c| c.is_ascii_alphanumeric() || " .()-".contains(*c)).collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `s` may be printed as it is.
fn fits(s: &str) -> bool { !s.is_empty() && s.len() <= MAX_LABEL && sanitise(s) == s }

/// `<spelling> (<detail>)`, within the alphabet and MAX_LABEL.
fn decorated(spelling: &str, detail: &str) -> String {
    let spelling: String = sanitise(spelling).chars().take(MAX_LABEL / 2).collect::<String>().trim().to_string();
    let room = MAX_LABEL.saturating_sub(spelling.len() + 3);
    let detail: String = sanitise(detail).chars().take(room).collect::<String>().trim().to_string();
    if spelling.is_empty() { format!("({detail})") } else { format!("{spelling} ({detail})") }
}

/// `base`, else `base 2`, `base 3` … within MAX_LABEL: the first `free` one.
fn counted(base: String, free: impl Fn(&str) -> bool) -> String {
    if free(&base) { return base; }
    (2..).map(|n: usize| { let tail = format!(" {n}"); format!("{}{tail}", base.chars().take(MAX_LABEL - tail.len()).collect::<String>().trim_end()) })
        .find(|s| free(s)).expect("a free counter")
}

struct Identity { holding: Holding, venue: String, uses: BTreeMap<String, usize> }

impl Spellings {
    /// The map for one owner: every leg of the whole history (pending ones too), its stored links, its current
    /// balances, and the catalogue's names. A quantity the arithmetic cannot hold is an error, never a skipped update.
    pub fn build(mut legs: Vec<LegIn>, links: &[Link], balances: &[BalanceIn], names: &Names) -> Result<Spellings, NumError> {
        // In time order; at one instant, as the transfers and each account's ledger order require (`ordered`).
        legs.sort_by(|a, b| (a.at, a.row, a.side).cmp(&(b.at, b.row, b.side)));
        // A link that cannot be a transfer never was one: the walk starts over without it, so nothing it ordered or
        // deferred survives (three rows at one instant: a deposit of X, an unclear sale, an unclear withdrawal linked
        // back to the deposit; the sale spends the X once the link is gone). A link dropped only because another, later
        // dropped, stood in its way comes back when the walk holds with it: each void link is offered back, in the
        // order it was dropped, until a round restores none.
        // ponytail: one whole walk per void link, and one per offer (void links are rare); replay from the link's
        // instant if histories ever carry many.
        let mut out: std::collections::HashSet<Link> = Default::default();
        let mut void: Vec<Link> = vec![];
        let live = |out: &std::collections::HashSet<Link>| -> Vec<Link> { links.iter().filter(|l| !out.contains(l)).copied().collect() };
        let mut done = loop {
            match walk(&legs, &live(&out), names)? { Ok(w) => break w, Err(link) => { out.insert(link); void.push(link); } }
        };
        loop {
            let mut restored = false;
            for link in void.clone() {
                out.remove(&link);
                match walk(&legs, &live(&out), names)? {
                    Ok(w) => { void.retain(|l| *l != link); done = w; restored = true; }
                    Err(_) => { out.insert(link); }
                }
            }
            if !restored { break; }
        }
        let Walked { legs, eff, keys, sources } = done;
        // The identities and their spellings, balances included (a balance of Y under X's spelling makes it two).
        let mut ids: BTreeMap<String, Identity> = BTreeMap::new();
        for ((l, h), k) in legs.iter().zip(&eff).zip(&keys) {
            // The lot's own holding, from its key: a transfer out of an unidentified lot holds the coin it was linked to,
            // and the lot it leaves stays unidentified.
            let own = if *k == key(l.account, &l.spelling, h) { h.clone() } else { UNCLEAR_HOLDING };
            let id = ids.entry(k.clone()).or_insert_with(|| Identity { holding: own, venue: l.venue.clone(), uses: BTreeMap::new() });
            // An unknown lot's reason: the strongest of its legs', whatever their order (Units < Cut < Conflict < Unclear).
            if let (Holding::Unknown(mine), Holding::Unknown(theirs)) = (&mut id.holding, h) { *mine = (*mine).min(*theirs); }
            *id.uses.entry(l.spelling.clone()).or_default() += 1;
        }
        for b in balances {
            ids.entry(key(b.account, &b.spelling, &b.holding)).or_insert_with(|| Identity { holding: b.holding.clone(), venue: b.venue.clone(), uses: BTreeMap::new() })
                .uses.entry(b.spelling.clone()).or_default();
        }
        let labels = allocate(&ids, names);
        let mut s = Spellings { void, ..Spellings::default() };
        for (k, id) in &ids {
            s.holdings.insert(labels[k].clone(), id.holding.clone());
            s.raw.insert(labels[k].clone(), id.uses.keys().cloned().collect());
        }
        for (i, l) in legs.iter().enumerate() { s.legs.insert((l.row, l.side), (labels[&keys[i]].clone(), eff[i].clone())); }
        for (a, (b, _)) in &sources { s.transfers.insert((legs[*a].row, legs[*a].side), (labels[&keys[*a]].clone(), labels[&keys[*b]].clone())); }
        for b in balances { s.assets.insert((b.account, b.asset_id), labels[&key(b.account, &b.spelling, &b.holding)].clone()); }
        Ok(s)
    }

    /// The label of a leg (a row's base, quote or fee): the lot its quantity is booked under.
    pub fn label(&self, row: i64, side: Side) -> Option<&Label> { self.legs.get(&(row, side)).map(|(l, _)| l) }
    /// What a leg holds after links and inheritance (a transfer out of an unclear lot holds what it was linked to, while
    /// its label stays the lot it leaves).
    pub fn leg_holding(&self, row: i64, side: Side) -> Option<&Holding> { self.legs.get(&(row, side)).map(|(_, h)| h) }
    /// What a label holds; an unknown label, the strongest reason among its legs.
    pub fn holding(&self, label: &Label) -> Option<&Holding> { self.holdings.get(label) }
    /// The owner's raw spellings behind a label, sorted: candidates for SQL that gathers by spelling (each row is then
    /// filtered by its own leg's label).
    pub fn raw(&self, label: &Label) -> &[String] { self.raw.get(label).map_or(&[], Vec::as_slice) }
    /// The label of an account's current balance of an asset.
    pub fn label_of_asset(&self, account: i64, asset_id: i64) -> Option<&Label> { self.assets.get(&(account, asset_id)) }
    pub fn class(&self, label: &Label) -> Class {
        match self.holdings.get(label) {
            Some(Holding::Cash(k)) => Class::Cash(*k),
            Some(Holding::Coin(_)) => Class::Coin,
            Some(Holding::Security(_)) => Class::Security,
            _ => Class::Unknown,
        }
    }
    /// A valid link's quantity move: the label it leaves and the label it arrives under. None for a void link.
    pub fn transfer(&self, link: &Link) -> Option<(&Label, &Label)> { self.transfers.get(&link.from).map(|(a, b)| (a, b)) }
    /// The links void for identity and quantity (`transfer_conflict`).
    pub fn void_links(&self) -> &[Link] { &self.void }
}

/// §1.4: one label per identity, deterministic, each within the alphabet and MAX_LABEL. Cash and securities print their
/// spelling. A coin prints its spelling when it has one that no other identity uses and that is no cash spelling; with
/// several, the most used (tie: the catalogue symbol, then the first). Otherwise, or when the spelling does not fit,
/// `<spelling> (<name>)`, then with its external id, then a counter, never equal to another label or to any raw
/// spelling. An unidentified lot prints its spelling unless a known identity uses it (or it does not fit), then
/// `<spelling> (unidentified on <venue>)`: the lots of one spelling on one venue's accounts print alike, and never like
/// a known label or another identity's spelling.
fn allocate(ids: &BTreeMap<String, Identity>, names: &Names) -> HashMap<String, Label> {
    let mut users: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    for (k, id) in ids { for s in id.uses.keys() { users.entry(s.as_str()).or_default().insert(k.as_str()); } }
    let raw: BTreeSet<&str> = users.keys().copied().collect();
    let mut taken: BTreeSet<String> = BTreeSet::new();
    let mut unknown: HashMap<String, (String, String)> = HashMap::new(); // printed → (spelling, venue)
    let mut out = HashMap::new();
    let rank = |h: &Holding| match h { Holding::Cash(_) | Holding::Security(_) => 0, Holding::Coin(_) => 1, Holding::Unknown(_) => 2 };
    let mut order: Vec<(&String, &Identity)> = ids.iter().collect();
    order.sort_by_key(|(k, id)| (rank(&id.holding), (*k).clone()));
    for (k, id) in order {
        let free = |s: &str, own: &str| !taken.contains(s) && !unknown.contains_key(s) && (s == own || !raw.contains(s));
        let ext = match &id.holding { Holding::Coin(e) | Holding::Security(e) => e.clone(), _ => String::new() };
        let (symbol, name) = names.get(&ext).cloned().unwrap_or_default();
        let generated = |best: &str| {
            let detail = name.clone().filter(|n| !sanitise(n).is_empty()).unwrap_or_else(|| ext.clone());
            let first = decorated(best, &detail);
            if free(&first, "") { return first; }
            counted(decorated(best, &format!("{detail} {ext}")), |s| free(s, ""))
        };
        let printed = match &id.holding {
            Holding::Cash(_) | Holding::Security(_) => {
                let best = id.uses.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map(|(s, _)| s.clone()).unwrap_or_default();
                if fits(&best) && free(&best, &best) { best } else { generated(&best) }
            }
            Holding::Coin(_) => {
                let best = id.uses.iter().max_by(|a, b| a.1.cmp(b.1).then((Some(a.0) == symbol.as_ref()).cmp(&(Some(b.0) == symbol.as_ref()))).then(b.0.cmp(a.0)))
                    .map(|(s, _)| s.clone()).unwrap_or_default();
                let reserved = super::cash(&best) || users.get(best.as_str()).is_some_and(|u| u.len() > 1);
                if !reserved && fits(&best) && free(&best, &best) { best } else { generated(&best) }
            }
            Holding::Unknown(_) => {
                let spelling = id.uses.keys().next().cloned().unwrap_or_default();
                let known = users.get(spelling.as_str()).is_some_and(|u| u.iter().any(|o| !o.starts_with("unknown:")));
                let mine = (spelling.clone(), id.venue.clone());
                let ours = |s: &str| !taken.contains(s) && !raw.contains(s) && unknown.get(s).is_none_or(|o| *o == mine);
                if !known && fits(&spelling) && !taken.contains(&spelling) && unknown.get(&spelling).is_none_or(|o| *o == mine) { spelling }
                else { counted(decorated(&spelling, &format!("unidentified on {}", id.venue)), ours) }
            }
        };
        match id.holding { Holding::Unknown(_) => { unknown.insert(printed.clone(), (id.uses.keys().next().cloned().unwrap_or_default(), id.venue.clone())); }
                           _ => { taken.insert(printed.clone()); } }
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

    /// PR1 step 1: a spelled label orders, prints, compares and stores as its spelling, so identity mode changes no
    /// output.
    #[test]
    fn identity_mode_label_is_its_spelling() {
        let mut v: Vec<Label> = ["eth", "BTC", "USD", "BTCB", ""].map(Label::from).to_vec();
        v.sort();
        assert_eq!(v.iter().map(|l| l.to_string()).collect::<Vec<_>>(), ["", "BTC", "BTCB", "USD", "eth"]);
        let (l, owned) = (Label::from("BTG"), String::from("BTG"));
        assert_eq!(l, "BTG");
        assert_eq!("BTG", l);
        assert_eq!(l, owned);
        assert_eq!(l.as_str(), "BTG");
        assert_eq!(String::from(l.clone()), owned);
        let c = rusqlite::Connection::open_in_memory().unwrap();
        let back: Label = c.query_row("SELECT ?1", [&l], |r| r.get(0)).unwrap();
        assert_eq!(back, l);
    }
    const UNCLEAR: Holding = Holding::Unknown(Why::Unclear);
    #[allow(clippy::too_many_arguments)] // a leg's fields
    fn leg(row: i64, side: Side, account: i64, venue: &str, spelling: &str, holding: Holding, day: &str, amount: &str) -> LegIn {
        LegIn { row, side, account, venue: venue.into(), spelling: spelling.into(), holding, at: at(day), amount: d(amount) }
    }
    fn base(row: i64, account: i64, spelling: &str, holding: Holding, day: &str, amount: &str) -> LegIn { leg(row, Side::Base, account, "Binance", spelling, holding, day, amount) }
    fn names(list: &[(&str, &str, &str)]) -> Names { list.iter().map(|(e, s, n)| (e.to_string(), (Some(s.to_string()), Some(n.to_string())))).collect() }
    fn build(legs: Vec<LegIn>, links: &[Link], balances: &[BalanceIn], names: &Names) -> Spellings { Spellings::build(legs, links, balances, names).unwrap() }
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
        let s = build(legs, &[], &[], &n);
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
        let s = build(legs, &[], &[], &n);
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
        let long = build(vec![base(1, 1, "S", coin("a"), "2024-01-01", "1"), base(2, 1, "S", coin("b"), "2024-01-01", "1")], &[], &[],
                                    &names(&[("a", "S", &"N".repeat(200))]));
        assert!(alphabet(&printed(&long, 1)) && printed(&long, 1).ends_with(')'), "{}", printed(&long, 1));
    }

    /// Codex P1-2 (r10): labels order by their printed string, then by identity: two accounts' unidentified `S` print
    /// alike and stay two keys; a known coin under `S` marks both as unidentified. A label survives a cache round trip.
    #[test]
    fn label_ord_breaks_ties_by_identity() {
        let legs = vec![base(1, 1, "S", UNCLEAR, "2024-01-01", "1"), base(2, 2, "S", UNCLEAR, "2024-01-02", "1")];
        let s = build(legs.clone(), &[], &[], &Names::new());
        let (a, b) = (s.label(1, Side::Base).unwrap().clone(), s.label(2, Side::Base).unwrap().clone());
        assert_eq!((a.to_string(), b.to_string()), ("S".into(), "S".into()));
        assert_ne!(a, b);
        assert_eq!(a.cmp(&b), std::cmp::Ordering::Less, "the identity breaks the tie");
        assert_eq!(s.class(&a), Class::Unknown);
        let mut more = legs;
        more.push(base(3, 1, "S", coin("s-coin"), "2024-01-03", "1"));
        let s = build(more, &[], &[], &Names::new());
        assert_eq!((printed(&s, 1), printed(&s, 3)), ("S (unidentified on Binance)".into(), "S (s-coin)".into()));
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(serde_json::from_str::<Label>(&json).unwrap(), a);
    }

    /// §1.3: an unidentified unit's key is its account's: an unclear buy on one account and an unclear sale on another
    /// are two identities; on one account, one.
    #[test]
    fn the_unknown_key_is_per_account() {
        let s = build(vec![base(1, 1, "S", UNCLEAR, "2024-01-01", "1"), base(2, 2, "S", UNCLEAR, "2024-02-01", "-1"),
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
        assert!(links_compatible(&UNCLEAR, "S", &UNCLEAR, "S", &Names::new()));
        assert!(!links_compatible(&UNCLEAR, "S", &UNCLEAR, "T", &Names::new()));
        assert!(links_compatible(&UNCLEAR, "S", &coin("x"), "S", &Names::new()) && links_compatible(&coin("x"), "X", &UNCLEAR, "S", &Names::new()));
        assert!(links_compatible(&coin("x"), "X", &coin("x"), "XX", &Names::new()), "one coin under two spellings");
        assert!(!links_compatible(&coin("x"), "S", &coin("y"), "S", &Names::new()));
        assert!(!links_compatible(&cut, "PHB", &coin("x"), "PHB", &Names::new()) && !links_compatible(&UNCLEAR, "PHB", &cut, "PHB", &Names::new()));
        assert!(!links_compatible(&units, "1000REKT", &coin("rekt"), "REKT", &Names::new()));
        assert!(!links_compatible(&Holding::Unknown(Why::Conflict), "S", &coin("x"), "S", &Names::new()));
        assert!(!links_compatible(&Holding::Cash(CashKind::Fiat), "EUR", &Holding::Cash(CashKind::Fiat), "USD", &Names::new()));
        // In the map: a known withdrawal makes its unclear deposit that coin; a void link is reported and carries nothing.
        let legs = vec![base(1, 1, "S", coin("x"), "2024-01-01", "1"), base(2, 1, "S", coin("x"), "2024-01-02", "-1"),
                        base(3, 2, "S", UNCLEAR, "2024-01-02", "1"),
                        base(4, 1, "1000REKT", units.clone(), "2024-01-03", "-1"), base(5, 2, "REKT", coin("rekt"), "2024-01-03", "1")];
        let links = [Link { from: (2, Side::Base), to: (3, Side::Base) }, Link { from: (4, Side::Base), to: (5, Side::Base) }];
        let s = build(legs, &links, &[], &Names::new());
        assert_eq!(s.label(3, Side::Base), s.label(1, Side::Base), "the deposit took the withdrawal's coin");
        assert_eq!(s.void_links(), &links[1..]);
        assert_eq!(s.holding(s.label(4, Side::Base).unwrap()), Some(&units));
        assert!(s.transfer(&links[1]).is_none());
        assert_eq!(s.transfer(&links[0]), Some((s.label(2, Side::Base).unwrap(), s.label(3, Side::Base).unwrap())));
    }

    /// §1.2 inheritance: an unclear outflow takes X only when X is the only identity held under its spelling at its
    /// account and covers it; X and Y both held, nothing held, or only unidentified units held leave it unknown; a hard
    /// refusal never inherits; an inflow never does; what it inherits crosses its link.
    #[test]
    fn inheritance_needs_exactly_one_source() {
        let s = build(vec![base(1, 1, "S", coin("x"), "2024-01-01", "2"), base(2, 1, "S", UNCLEAR, "2024-01-02", "-1"),
                                      base(3, 1, "S", UNCLEAR, "2024-01-03", "-5"), base(4, 1, "S", UNCLEAR, "2024-01-04", "1")], &[], &[], &Names::new());
        assert_eq!(s.holding(s.label(2, Side::Base).unwrap()), Some(&coin("x")), "X alone covers it");
        assert_eq!(s.class(s.label(3, Side::Base).unwrap()), Class::Unknown, "more than X holds");
        assert_eq!(s.class(s.label(4, Side::Base).unwrap()), Class::Unknown, "an inflow");
        let both = build(vec![base(1, 1, "S", coin("x"), "2024-01-01", "2"), base(2, 1, "S", coin("y"), "2024-01-02", "2"),
                                         base(3, 1, "S", UNCLEAR, "2024-01-03", "-1")], &[], &[], &Names::new());
        assert_eq!(both.class(both.label(3, Side::Base).unwrap()), Class::Unknown, "X and Y both held");
        let gone = build(vec![base(1, 1, "S", coin("x"), "2024-01-01", "1"), base(2, 1, "S", coin("x"), "2024-01-02", "-1"),
                                         base(3, 1, "S", UNCLEAR, "2024-01-03", "1"), base(4, 1, "S", UNCLEAR, "2024-01-04", "-1")], &[], &[], &Names::new());
        assert_eq!(gone.class(gone.label(4, Side::Base).unwrap()), Class::Unknown, "only unidentified units are held");
        let other = build(vec![base(1, 1, "S", coin("x"), "2024-01-01", "2"), base(2, 2, "S", UNCLEAR, "2024-01-02", "-1")], &[], &[], &Names::new());
        assert_eq!(other.class(other.label(2, Side::Base).unwrap()), Class::Unknown, "X is held at another account");
        let cut = build(vec![base(1, 1, "PHB", coin("x"), "2024-01-01", "2"), base(2, 1, "PHB", Holding::Unknown(Why::Cut), "2024-01-02", "-1")],
                                   &[], &[], &Names::new());
        assert_eq!(cut.holding(cut.label(2, Side::Base).unwrap()), Some(&Holding::Unknown(Why::Cut)), "a hard refusal never inherits");
        let linked = build(vec![base(1, 1, "S", coin("x"), "2024-01-01", "2"), base(2, 1, "S", UNCLEAR, "2024-01-02", "-1"),
                                           base(3, 2, "S", UNCLEAR, "2024-01-02", "1")], &[Link { from: (2, Side::Base), to: (3, Side::Base) }], &[], &Names::new());
        assert_eq!(linked.holding(linked.label(3, Side::Base).unwrap()), Some(&coin("x")), "the inherited coin crosses the link");
    }

    /// Balances are an input: a balance of Y under the spelling the history uses for X makes the spelling two coins, and
    /// the balance's asset finds its label.
    #[test]
    fn a_balance_under_a_historys_spelling_splits_it() {
        let balances = [BalanceIn { account: 1, venue: "Binance".into(), asset_id: 77, spelling: "S".into(), holding: coin("y") }];
        let s = build(vec![base(1, 1, "S", coin("x"), "2024-01-01", "1")], &[], &balances, &names(&[("x", "S", "Ex"), ("y", "S", "Why")]));
        assert_eq!(printed(&s, 1), "S (Ex)");
        assert_eq!(s.label_of_asset(1, 77).unwrap().to_string(), "S (Why)");
        assert_eq!(s.label_of_asset(1, 78), None);
    }

    /// Codex PR0 P2-9: what an account holds of an identity counts, whatever spelling moved it: one X bought under OLD and
    /// sold under NEW leaves none for an unclear OLD outflow to inherit.
    #[test]
    fn inheritance_counts_what_the_account_holds_of_the_identity() {
        let s = build(vec![base(1, 1, "OLD", coin("x"), "2024-01-01", "1"), base(2, 1, "NEW", coin("x"), "2024-01-02", "-1"),
                                      base(3, 1, "OLD", UNCLEAR, "2024-01-03", "-1")], &[], &[], &Names::new());
        assert_eq!(s.class(s.label(3, Side::Base).unwrap()), Class::Unknown);
    }

    /// Codex PR0 P2-10: a link never carries another unit's quantity (an unclear `1000REKT` against a known `REKT`), and
    /// cash links only with the same cash: a known `USD` gives an unclear `S` nothing.
    #[test]
    fn links_check_units_and_cash_identity() {
        assert!(!links_compatible(&UNCLEAR, "1000REKT", &coin("rekt"), "REKT", &Names::new()));
        assert!(!links_compatible(&coin("rekt"), "REKT", &UNCLEAR, "1000REKT", &Names::new()));
        assert!(!links_compatible(&Holding::Cash(CashKind::Fiat), "USD", &UNCLEAR, "S", &Names::new()));
        assert!(links_compatible(&Holding::Cash(CashKind::Fiat), "USD", &Holding::Cash(CashKind::Fiat), "USD", &Names::new()));
    }

    /// Codex PR0 P2-12: every label is checked, not only decorated ones: a spelling outside the alphabet or over the bound
    /// is sanitised, and an unidentified label never prints as another identity's raw spelling.
    #[test]
    fn every_label_fits_the_alphabet_and_collides_with_nothing() {
        let long = "L".repeat(70);
        let legs = vec![base(1, 1, "A/B", coin("ab"), "2024-01-01", "1"), base(2, 1, &long, coin("long"), "2024-01-02", "1"),
                        base(3, 1, "S", coin("s-coin"), "2024-01-03", "1"), base(4, 1, "S", UNCLEAR, "2024-01-04", "1"),
                        base(5, 1, "S (unidentified on Binance)", coin("tricky"), "2024-01-05", "1")];
        let s = build(legs, &[], &[], &Names::new());
        let all: Vec<String> = (1..=5).map(|r| printed(&s, r)).collect();
        for l in &all { assert!(alphabet(l) && !l.is_empty(), "{l:?}"); }
        assert_eq!(printed(&s, 5), "S (unidentified on Binance)", "a valid raw spelling of one coin is its label");
        assert_ne!(printed(&s, 4), printed(&s, 5), "the unidentified label takes another form");
        assert_eq!(all.iter().collect::<BTreeSet<_>>().len(), 5, "{all:?}");
    }

    /// Codex PR0 P2-11: an unknown label's holding does not depend on the order its legs came in.
    #[test]
    fn an_unknown_labels_holding_is_order_free() {
        let legs = vec![base(1, 1, "S", Holding::Unknown(Why::Conflict), "2024-01-01", "1"), base(2, 1, "S", UNCLEAR, "2024-01-01", "1")];
        let mut flipped = legs.clone();
        flipped.reverse();
        flipped[0].row = 0; // the unclear leg now sorts first
        let (a, b) = (build(legs, &[], &[], &Names::new()), build(flipped, &[], &[], &Names::new()));
        assert_eq!(a.holding(a.label(1, Side::Base).unwrap()), b.holding(b.label(1, Side::Base).unwrap()));
    }

    /// Codex PR0 P1-5: a transfer out of an unclear leg moves the lot its account holds under the spelling, not the coin
    /// it is linked to: A's unidentified S leaves A's unidentified lot and arrives at B as X. With X and Y both held under
    /// S at A, or nothing held there, the link is void.
    #[test]
    fn link_unknown_to_known_moves_basis_from_source_key() {
        let link = Link { from: (2, Side::Base), to: (3, Side::Base) };
        let s = build(vec![base(1, 1, "S", UNCLEAR, "2024-01-01", "1"), base(2, 1, "S", UNCLEAR, "2024-01-02", "-1"), base(3, 2, "S", coin("x"), "2024-01-02", "1")],
                      &[link], &[], &Names::new());
        let (out, into) = s.transfer(&link).unwrap();
        assert_eq!((out, into), (s.label(1, Side::Base).unwrap(), s.label(3, Side::Base).unwrap()), "from A's unidentified lot to X");
        assert_eq!((s.class(out), s.class(into)), (Class::Unknown, Class::Coin));
        assert_eq!(s.leg_holding(2, Side::Base), Some(&coin("x")), "the withdrawal holds what it was linked to");
        assert!(s.void_links().is_empty());
        for held in [vec![base(1, 1, "S", coin("x"), "2024-01-01", "1"), base(4, 1, "S", coin("y"), "2024-01-01", "1")], vec![]] {
            let mut legs = held;
            legs.extend([base(2, 1, "S", UNCLEAR, "2024-01-02", "-1"), base(3, 2, "S", coin("x"), "2024-01-02", "1")]);
            let s = build(legs, &[link], &[], &Names::new());
            assert_eq!(s.void_links(), &[link]);
            assert!(s.transfer(&link).is_none());
            assert_eq!(s.class(s.label(2, Side::Base).unwrap()), Class::Unknown, "no identity crosses a void link");
        }
    }

    /// Codex PR0 P2-11: a balance is the account's: two accounts' unidentified balances of one asset are two labels; each
    /// leg keeps its own holding while the unknown legs of one account and spelling share their lot.
    #[test]
    fn balances_by_account_and_holdings_per_leg() {
        let b = |account| BalanceIn { account, venue: "Binance".into(), asset_id: 77, spelling: "S".into(), holding: UNCLEAR };
        let s = build(vec![], &[], &[b(1), b(2)], &Names::new());
        assert_ne!(s.label_of_asset(1, 77), s.label_of_asset(2, 77));
        let s = build(vec![base(1, 1, "S", Holding::Unknown(Why::Conflict), "2024-01-01", "1"), base(2, 1, "S", UNCLEAR, "2024-01-02", "1")], &[], &[], &Names::new());
        assert_eq!(s.label(1, Side::Base), s.label(2, Side::Base), "one lot");
        assert_eq!((s.leg_holding(1, Side::Base), s.leg_holding(2, Side::Base)), (Some(&Holding::Unknown(Why::Conflict)), Some(&UNCLEAR)));
        assert_eq!(s.holding(s.label(2, Side::Base).unwrap()), Some(&Holding::Unknown(Why::Conflict)), "the lot: the strongest reason");
    }

    /// Codex PR0 r2 P1-6: the lot an unclear transfer leaves must itself be compatible with the deposit and cover the
    /// amount: A holding only Y under S never sends B's X; two units out of a one-unit lot void the link.
    #[test]
    fn an_unclear_transfer_never_links_two_known_coins_or_more_than_its_lot() {
        let link = Link { from: (2, Side::Base), to: (3, Side::Base) };
        let y = build(vec![base(1, 1, "S", coin("y"), "2024-01-01", "1"), base(2, 1, "S", UNCLEAR, "2024-01-02", "-1"), base(3, 2, "S", coin("x"), "2024-01-02", "1")],
                      &[link], &[], &Names::new());
        assert_eq!(y.void_links(), &[link]);
        assert!(y.transfer(&link).is_none());
        let short = build(vec![base(1, 1, "S", UNCLEAR, "2024-01-01", "1"), base(2, 1, "S", UNCLEAR, "2024-01-02", "-2"), base(3, 2, "S", coin("x"), "2024-01-02", "2")],
                          &[link], &[], &Names::new());
        assert_eq!(short.void_links(), &[link]);
    }

    /// Codex PR0 r2 P2-11: the unit check knows a coin's own symbol (Kraken's XBT is Bitcoin, so `1000BTC` is another
    /// unit of it); at one instant, each transfer's source comes before its own destination and other legs keep ledger
    /// order, so A → B → C finds B's deposit before B's withdrawal.
    #[test]
    fn links_check_the_coins_symbol_and_keep_ledger_order() {
        let link = Link { from: (2, Side::Base), to: (3, Side::Base) };
        let n = names(&[("bitcoin", "BTC", "Bitcoin")]);
        let s = build(vec![base(1, 1, "1000BTC", UNCLEAR, "2024-01-01", "1"), base(2, 1, "1000BTC", UNCLEAR, "2024-01-02", "-1"),
                           base(3, 2, "XBT", coin("bitcoin"), "2024-01-02", "1")], &[link], &[], &n);
        assert_eq!(s.void_links(), &[link]);
        let (ab, bc) = (Link { from: (2, Side::Base), to: (3, Side::Base) }, Link { from: (4, Side::Base), to: (5, Side::Base) });
        let s = build(vec![base(1, 1, "S", coin("x"), "2024-01-01", "1"), base(2, 1, "S", UNCLEAR, "2024-01-02", "-1"), base(3, 2, "S", UNCLEAR, "2024-01-02", "1"),
                           base(4, 2, "S", UNCLEAR, "2024-01-02", "-1"), base(5, 3, "S", UNCLEAR, "2024-01-02", "1")], &[ab, bc], &[], &Names::new());
        assert!(s.void_links().is_empty(), "{:?}", s.void_links());
        assert_eq!(s.holding(s.label(5, Side::Base).unwrap()), Some(&coin("x")), "X reached C");
    }

    /// Codex PR0 r2 P3-13: an unidentified balance on an account with no history carries its venue into its label.
    #[test]
    fn a_balance_only_unknown_names_its_venue() {
        let balances = [BalanceIn { account: 9, venue: "Kraken".into(), asset_id: 5, spelling: "S".into(), holding: UNCLEAR }];
        let s = build(vec![base(1, 1, "S", coin("x"), "2024-01-01", "1")], &[], &balances, &Names::new());
        assert_eq!(s.label_of_asset(9, 5).unwrap().to_string(), "S (unidentified on Kraken)");
    }

    /// Codex PR0 r3 P1-3: a transfer voided after its lot is chosen is still an outflow: A's one Y leaves with it (the
    /// withdrawal inherits Y), so a later unclear sale at A finds nothing to inherit.
    #[test]
    fn a_void_transfer_spends_its_lot_as_an_ordinary_outflow() {
        let link = Link { from: (2, Side::Base), to: (3, Side::Base) };
        let s = build(vec![base(1, 1, "S", coin("y"), "2024-01-01", "1"), base(2, 1, "S", UNCLEAR, "2024-01-02", "-1"), base(3, 2, "S", coin("x"), "2024-01-02", "1"),
                           base(4, 1, "S", UNCLEAR, "2024-01-03", "-1")], &[link], &[], &Names::new());
        assert_eq!(s.void_links(), &[link]);
        assert_eq!(s.leg_holding(2, Side::Base), Some(&coin("y")), "the withdrawal took A's Y");
        assert_eq!(s.label(2, Side::Base), s.label(1, Side::Base));
        assert_eq!(s.class(s.label(4, Side::Base).unwrap()), Class::Unknown, "nothing left to inherit");
    }

    /// Codex PR0 r3 P1-4: the refusal travels with the units outstanding: an unclear lot spent, then a conflicting unit
    /// deposited under the same lot, then an unclear withdrawal linked to a known X: the link is void.
    #[test]
    fn a_refused_lot_never_crosses_a_link() {
        let link = Link { from: (4, Side::Base), to: (5, Side::Base) };
        let s = build(vec![base(1, 1, "S", UNCLEAR, "2024-01-01", "1"), base(2, 1, "S", UNCLEAR, "2024-01-02", "-1"),
                           base(3, 1, "S", Holding::Unknown(Why::Conflict), "2024-01-03", "1"), base(4, 1, "S", UNCLEAR, "2024-01-04", "-1"),
                           base(5, 2, "S", coin("x"), "2024-01-04", "1")], &[link], &[], &Names::new());
        assert_eq!(s.void_links(), &[link]);
        assert!(s.transfer(&link).is_none());
    }

    /// Codex PR0 r3 P2-8: at one instant the order follows the transfers and each account's ledger order: B's deposit
    /// (row 2) before B's onward withdrawal (row 3), A's funding withdrawal (row 4) before B's deposit.
    #[test]
    fn equal_time_transfers_follow_their_dependencies() {
        let (ab, bc) = (Link { from: (4, Side::Base), to: (2, Side::Base) }, Link { from: (3, Side::Base), to: (5, Side::Base) });
        let s = build(vec![base(1, 1, "S", coin("x"), "2024-01-01", "1"), base(2, 2, "S", UNCLEAR, "2024-01-02", "1"), base(3, 2, "S", UNCLEAR, "2024-01-02", "-1"),
                           base(4, 1, "S", UNCLEAR, "2024-01-02", "-1"), base(5, 3, "S", UNCLEAR, "2024-01-02", "1")], &[ab, bc], &[], &Names::new());
        assert!(s.void_links().is_empty(), "{:?}", s.void_links());
        assert_eq!(s.leg_holding(5, Side::Base), Some(&coin("x")), "X reached C");
    }

    /// Codex PR0 r4 P1: a linked transfer lands where it was sent at the withdrawal, as the tracker books it: B, holding
    /// Y, receives A's X at 10:00 (deposit booked 10:02), so B's unclear sale at 10:01 sees X and Y and inherits neither.
    #[test]
    fn a_transfer_lands_at_the_withdrawal() {
        let t = |h: &str| -> DateTime<Utc> { format!("2024-01-02T{h}:00Z").parse().unwrap() };
        let at = |mut l: LegIn, h: &str| { l.at = t(h); l };
        let link = Link { from: (2, Side::Base), to: (4, Side::Base) };
        let s = build(vec![base(1, 2, "S", coin("y"), "2024-01-01", "1"), base(5, 1, "S", coin("x"), "2024-01-01", "1"),
                           at(base(2, 1, "S", coin("x"), "2024-01-02", "-1"), "10:00"), at(base(3, 2, "S", UNCLEAR, "2024-01-02", "-1"), "10:01"),
                           at(base(4, 2, "S", UNCLEAR, "2024-01-02", "1"), "10:02")], &[link], &[], &Names::new());
        assert!(s.void_links().is_empty());
        assert_eq!(s.class(s.label(3, Side::Base).unwrap()), Class::Unknown, "X and Y both held at B");
        assert_eq!(s.leg_holding(4, Side::Base), Some(&coin("x")));
    }

    /// Codex PR0 r4 P1: two transfers crossing at one instant (A sends S to B, B sends T to A; both deposits on lower rows)
    /// book each deposit under the coin its withdrawal moved, label and holding alike.
    #[test]
    fn crossing_transfers_at_one_instant_book_their_coins() {
        let (ab, ba) = (Link { from: (5, Side::Base), to: (4, Side::Base) }, Link { from: (6, Side::Base), to: (3, Side::Base) });
        let s = build(vec![base(1, 1, "S", coin("x"), "2024-01-01", "1"), base(2, 2, "T", coin("z"), "2024-01-01", "1"),
                           base(3, 1, "T", UNCLEAR, "2024-01-02", "1"), base(4, 2, "S", UNCLEAR, "2024-01-02", "1"),
                           base(5, 1, "S", UNCLEAR, "2024-01-02", "-1"), base(6, 2, "T", UNCLEAR, "2024-01-02", "-1")], &[ab, ba], &[], &Names::new());
        assert!(s.void_links().is_empty(), "{:?}", s.void_links());
        assert_eq!((s.leg_holding(4, Side::Base), s.label(4, Side::Base)), (Some(&coin("x")), s.label(1, Side::Base)));
        assert_eq!((s.leg_holding(3, Side::Base), s.label(3, Side::Base)), (Some(&coin("z")), s.label(2, Side::Base)));
    }

    /// Codex PR0 r5 P1: a lot's holding comes from its own key: A's unidentified S reaches B (deposit booked 10:02), and
    /// at 10:01 B forwards it to C, whose deposit names X. B's withdrawal holds X; B's lot, and the deposit booked under
    /// it, stay unidentified (never priced as X).
    #[test]
    fn a_forwarded_unknown_lot_stays_unidentified() {
        let t = |l: LegIn, h: &str| LegIn { at: format!("2024-01-02T{h}:00Z").parse().unwrap(), ..l };
        let (ab, bc) = (Link { from: (2, Side::Base), to: (4, Side::Base) }, Link { from: (3, Side::Base), to: (5, Side::Base) });
        let s = build(vec![base(1, 1, "S", UNCLEAR, "2024-01-01", "1"), t(base(2, 1, "S", UNCLEAR, "2024-01-02", "-1"), "10:00"),
                           t(base(3, 2, "S", UNCLEAR, "2024-01-02", "-1"), "10:01"), t(base(4, 2, "S", UNCLEAR, "2024-01-02", "1"), "10:02"),
                           t(base(5, 3, "S", coin("x"), "2024-01-02", "1"), "10:01")], &[ab, bc], &[], &Names::new());
        assert!(s.void_links().is_empty(), "{:?}", s.void_links());
        let lot = s.label(4, Side::Base).unwrap();
        assert_eq!(s.label(3, Side::Base), Some(lot), "the withdrawal leaves B's lot");
        assert_eq!((s.class(lot), s.holding(lot)), (Class::Unknown, Some(&UNCLEAR)));
        assert_eq!(s.leg_holding(3, Side::Base), Some(&coin("x")), "the outgoing leg holds what it was linked to");
        assert_eq!(s.class(s.label(5, Side::Base).unwrap()), Class::Coin);
    }

    /// Codex PR0 r5 P1: a link that voids is no transfer at any point of the walk: on an empty account at one instant, a
    /// deposit of X, an unclear sale, and an unclear withdrawal linked back to the deposit. Without the link the deposit
    /// keeps its ledger place, the sale spends the X, and a later unclear sale finds nothing to inherit.
    #[test]
    fn a_void_link_never_defers_its_deposit() {
        let link = Link { from: (3, Side::Base), to: (1, Side::Base) };
        let s = build(vec![base(1, 1, "S", coin("x"), "2024-01-02", "1"), base(2, 1, "S", UNCLEAR, "2024-01-02", "-1"), base(3, 1, "S", UNCLEAR, "2024-01-02", "-1"),
                           base(4, 1, "S", UNCLEAR, "2024-01-03", "-1")], &[link], &[], &Names::new());
        assert_eq!(s.void_links(), &[link]);
        assert_eq!(s.leg_holding(2, Side::Base), Some(&coin("x")), "the first sale spent the deposit");
        assert_eq!(s.class(s.label(3, Side::Base).unwrap()), Class::Unknown);
        assert_eq!(s.class(s.label(4, Side::Base).unwrap()), Class::Unknown, "nothing left for the later sale");
    }

    /// Codex PR1a r1 P1: a link whose ends are incompatible orders nothing: at one instant, a deposit of Z at A (row 1),
    /// A sends X to B (row 2), B's unclear sale (row 3), and C's withdrawal of Y (row 4) linked, incompatibly, to row 1.
    /// B's sale spends the X.
    #[test]
    fn an_incompatible_link_orders_nothing() {
        let (ab, bad) = (Link { from: (2, Side::Base), to: (5, Side::Base) }, Link { from: (4, Side::Base), to: (1, Side::Base) });
        let s = build(vec![base(9, 1, "S", coin("x"), "2024-01-01", "1"), base(1, 1, "Z", coin("z"), "2024-01-02", "1"), base(2, 1, "S", coin("x"), "2024-01-02", "-1"),
                           base(3, 2, "S", UNCLEAR, "2024-01-02", "-1"), base(4, 3, "Y", coin("y"), "2024-01-02", "-1"), base(5, 2, "S", UNCLEAR, "2024-01-02", "1")],
                      &[ab, bad], &[], &Names::new());
        assert_eq!(s.void_links(), &[bad]);
        assert_eq!(s.leg_holding(3, Side::Base), Some(&coin("x")));
    }

    /// Codex PR1a r1 P1: a link dropped only because another void link stood in its way comes back: B holds Y; at one
    /// instant A and B each have a deposit of X and an unclear withdrawal linked to the other's deposit. B's withdrawal
    /// is void (X and Y held); without it A's deposit lands, and A's withdrawal carries that X to B.
    #[test]
    fn a_link_another_void_link_blocked_is_offered_back() {
        let (ab, ba) = (Link { from: (2, Side::Base), to: (3, Side::Base) }, Link { from: (4, Side::Base), to: (1, Side::Base) });
        let s = build(vec![base(9, 2, "S", coin("y"), "2024-01-01", "1"), base(1, 1, "S", coin("x"), "2024-01-02", "1"), base(2, 1, "S", UNCLEAR, "2024-01-02", "-1"),
                           base(3, 2, "S", coin("x"), "2024-01-02", "1"), base(4, 2, "S", UNCLEAR, "2024-01-02", "-1")], &[ab, ba], &[], &Names::new());
        assert_eq!(s.void_links(), &[ba]);
        assert_eq!(s.leg_holding(2, Side::Base), Some(&coin("x")), "A's withdrawal carried its X");
        assert!(s.transfer(&ab).is_some());
    }

    /// Codex PR1a r2 P1: the offer goes round until nothing comes back. A holds one X; at one instant under `S`: C
    /// deposits Y (1), C withdraws (2 → 5), B withdraws (3 → 7), A sells (4), B deposits (5), B withdraws (6 → 1), A
    /// deposits Y (7). Only 6 → 1 is void; Y travels C → B → A before A's sale, which then sees X and Y and takes neither.
    #[test]
    fn void_links_are_offered_back_until_none_returns() {
        let l = |a, b| Link { from: (a, Side::Base), to: (b, Side::Base) };
        let links = [l(6, 1), l(2, 5), l(3, 7)];
        let s = build(vec![base(9, 1, "S", coin("x"), "2024-01-01", "1"), base(1, 3, "S", coin("y"), "2024-01-02", "1"), base(2, 3, "S", UNCLEAR, "2024-01-02", "-1"),
                           base(3, 2, "S", UNCLEAR, "2024-01-02", "-1"), base(4, 1, "S", UNCLEAR, "2024-01-02", "-1"), base(5, 2, "S", UNCLEAR, "2024-01-02", "1"),
                           base(6, 2, "S", UNCLEAR, "2024-01-02", "-1"), base(7, 1, "S", coin("y"), "2024-01-02", "1")], &links, &[], &Names::new());
        assert_eq!(s.void_links(), &links[..1]);
        assert!(s.transfer(&links[1]).is_some() && s.transfer(&links[2]).is_some());
        assert_eq!(s.class(s.label(4, Side::Base).unwrap()), Class::Unknown, "X and Y both held at A");
    }
}
