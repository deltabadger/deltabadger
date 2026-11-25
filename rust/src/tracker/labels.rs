//! Delisted coins (plan r10 PR1 step 2): the owner's `Spellings`, read from the store. Every leg of the whole history
//! (base, quote and fee of each row `rows::load_all` reads, before the splits and any year filter), its stored links,
//! the current balances and the catalogue's names go into one map; the resolver reads the tickers of the owner's venues
//! and the catalogue's assets under the owner's spellings.
use super::resolver::{self, AssetRef, Facts, Side, VenueRef};
use super::rows::{Kind, Stored};
use super::spellings::{BalanceIn, LegIn, Link, Names, Spellings};
use crate::figures::{budget::charge, dec::Dec, FiguresError};
use rusqlite::Connection;
use std::collections::{BTreeSet, HashMap, HashSet};

/// The assets whose `column` is one of `values`, by id, each a step of the open budget scope.
fn assets_where(c: &Connection, ok: (bool, bool), column: &str, values: &BTreeSet<String>) -> Result<Vec<(AssetRef, Option<String>)>, FiguresError> {
    let mut out = vec![];
    let (ok, named) = ok;
    if !ok { return Ok(out); }
    let name = if named { "name" } else { "NULL" };
    let values: Vec<&String> = values.iter().collect();
    for chunk in values.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(", ");
        let mut s = c.prepare(&format!("SELECT id, external_id, symbol, category, {name} FROM assets WHERE {column} IN ({marks}) ORDER BY id"))?;
        let mut q = s.query(rusqlite::params_from_iter(chunk))?;
        while let Some(r) = q.next()? {
            charge(1, 0)?;
            out.push((AssetRef { id: r.get(0)?, external_id: r.get(1)?, symbol: r.get(2)?, category: r.get(3)? }, r.get(4)?));
        }
    }
    Ok(out)
}

/// A row's legs with their signed quantities (in > 0, out < 0): the base by the row's kind, the quote of a trade the
/// other way, the fee out. A leg with no spelling is none (a blank quote).
fn legs_of(r: &Stored) -> Vec<(Side, String, Dec)> {
    let zero = Dec::zero();
    let mut out = vec![];
    let base = if r.kind.base_in() { r.amount.clone() } else if r.kind.base_out() { r.amount.neg() } else { zero.clone() };
    out.push((Side::Base, r.base.as_str().to_string(), base));
    if let Some(q) = r.quote.as_ref() {
        let paid = r.quote_amount.clone().unwrap_or_else(Dec::zero);
        let signed = match r.kind { Kind::Buy => paid.neg(), Kind::Sell => paid, _ => zero.clone() };
        out.push((Side::Quote, q.as_str().to_string(), signed));
    }
    if let Some(f) = r.fee_currency.as_ref() { out.push((Side::Fee, f.as_str().to_string(), r.fee_amount.clone().unwrap_or_else(Dec::zero).neg())); }
    out.retain(|(_, s, _)| !s.trim().is_empty());
    out
}

/// The owner's map over `rows` (every row `load_all` reads, borrowed wallets' included), read before they are relabelled.
pub fn load(c: &Connection, user_id: i64, rows: &[Stored]) -> Result<Spellings, FiguresError> {
    // The venues: the rows' and the balances'.
    // Each venue's type, and the name an unidentified label prints (its display name, else its name id).
    let named = c.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('exchanges') WHERE name = 'name')", [], |r| r.get::<_, bool>(0))?;
    let mut s = c.prepare(if named { "SELECT id, type, name FROM exchanges ORDER BY id" } else { "SELECT id, type, NULL FROM exchanges ORDER BY id" })?;
    let kinds: HashMap<i64, (Option<String>, Option<String>)> = s.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?.collect::<Result<_, _>>()?;
    let venue = |id: i64| {
        let t = kinds.get(&id).and_then(|k| k.0.clone()).unwrap_or_default();
        VenueRef { id, name_id: super::rows::name_id(&t), broker: super::prices::STOCK_TYPES.contains(&t.as_str()) }
    };
    let shown = |v: &VenueRef| kinds.get(&v.id).and_then(|k| k.1.clone()).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| v.name_id.clone());
    // The balances, by account (as the rows' account: the membership's, else minus the exchange id).
    // A store without a table or column the map reads (an old schema, a test's) reads none of it.
    let has = |table: &str, cols: &[&str]| -> rusqlite::Result<bool> {
        let n: i64 = c.query_row(&format!("SELECT count(*) FROM pragma_table_info('{table}') WHERE name IN ({})", cols.iter().map(|c| format!("'{c}'")).collect::<Vec<_>>().join(", ")), [], |r| r.get(0))?;
        Ok(n == cols.len() as i64)
    };
    let assets_ok = has("assets", &["id", "external_id", "symbol", "category"])?;
    let catalogue = (assets_ok, has("assets", &["name"])?);
    let balances_ok = assets_ok && has("exchange_account_balances", &["user_id", "exchange_id", "asset_id", "free", "locked"])?;
    let tickers_ok = assets_ok && has("tickers", &["exchange_id", "base", "base_asset_id"])?;
    let quoted = has("tickers", &["quote", "quote_asset_id"])?;
    let has_account = has("exchange_account_balances", &["account_id"])?;
    let mut balances_raw: Vec<(i64, i64, AssetRef)> = vec![];
    if balances_ok {
        let (account, counted) = if has_account {
            ("COALESCE(b.account_id, -b.exchange_id)".to_string(), crate::identity::counted(c, user_id, "b.account_id")?)
        } else { ("-b.exchange_id".to_string(), "1".to_string()) };
        let mut s = c.prepare(&format!("SELECT {account}, b.exchange_id, a.id, a.external_id, a.symbol, a.category FROM exchange_account_balances b \
                                        JOIN assets a ON a.id = b.asset_id WHERE b.user_id = ?1 AND {counted} AND b.free + b.locked > 0 ORDER BY b.id"))?;
        let mut q = s.query([user_id])?;
        while let Some(r) = q.next()? {
            charge(1, 0)?;
            balances_raw.push((r.get(0)?, r.get(1)?, AssetRef { id: r.get(2)?, external_id: r.get(3)?, symbol: r.get(4)?, category: r.get(5)? }));
        }
    }
    let venues: BTreeSet<i64> = rows.iter().map(|r| r.exchange_id).chain(balances_raw.iter().map(|b| b.1)).collect();
    // The facts: each venue's live listings by side, its tombstoned bases, the catalogue under the owner's spellings.
    let mut f = Facts::default();
    let venues: Vec<&i64> = if tickers_ok { venues.iter().collect() } else { vec![] };
    for chunk in venues.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(", ");
        let mut s = c.prepare(&format!(
            "SELECT t.exchange_id, t.base, {}, b.id, b.external_id, b.symbol, b.category, q.id, q.external_id, q.symbol, q.category FROM tickers t \
             LEFT JOIN assets b ON b.id = t.base_asset_id LEFT JOIN assets q ON q.id = {} WHERE t.exchange_id IN ({marks}) ORDER BY t.id",
            if quoted { "t.quote" } else { "''" }, if quoted { "t.quote_asset_id" } else { "NULL" }))?;
        let mut q = s.query(rusqlite::params_from_iter(chunk))?;
        while let Some(r) = q.next()? {
            charge(1, 0)?;
            let (v, base, quote): (i64, String, String) = (r.get(0)?, r.get(1)?, r.get(2)?);
            let asset = |i: usize| -> rusqlite::Result<Option<AssetRef>> {
                r.get::<_, Option<i64>>(i)?.map(|id| -> rusqlite::Result<AssetRef> { Ok(AssetRef { id, external_id: r.get(i + 1)?, symbol: r.get(i + 2)?, category: r.get(i + 3)? }) }).transpose()
            };
            let spelling = crate::figures::db::base_spelling(&base);
            if spelling != base {
                if let Some(a) = asset(3)? { f.stale.entry((v, spelling.to_string())).or_default().push(a); }
                continue;
            }
            if let Some(a) = asset(3)? { f.live.entry((v, Side::Base, base)).or_default().push(a); }
            if let Some(a) = asset(7)? { f.live.entry((v, Side::Quote, quote)).or_default().push(a); }
        }
    }
    let mut legs: Vec<(usize, Side, String, Dec)> = vec![];
    for (i, r) in rows.iter().enumerate() { for (side, spelling, amount) in legs_of(r) { legs.push((i, side, spelling, amount)); } }
    let spellings: BTreeSet<String> = legs.iter().map(|l| l.2.clone()).chain(balances_raw.iter().filter_map(|b| b.2.symbol.clone())).collect();
    for (a, _) in assets_where(c, catalogue, "symbol", &spellings)? {
        if let Some(symbol) = a.symbol.clone() { f.catalogue.entry(symbol).or_default().push(a); }
    }
    let recorded_ids: BTreeSet<String> = rows.iter().filter_map(|r| r.base_asset.as_ref().map(|a| a.0.to_string())).collect();
    let recorded: HashMap<i64, AssetRef> = assets_where(c, catalogue, "id", &recorded_ids)?.into_iter().map(|(a, _)| (a.id, a)).collect();
    // Each leg's holding.
    let mut ins = Vec::with_capacity(legs.len());
    for (i, side, spelling, amount) in legs {
        charge(1, 0)?;
        let r = &rows[i];
        let v = venue(r.exchange_id);
        let at = r.at.utc();
        let rec = (side == Side::Base).then(|| r.base_asset.as_ref().and_then(|a| recorded.get(&a.0))).flatten();
        let holding = resolver::holding(&f, &resolver::Leg { venue: &v, side, spelling: &spelling, at, recorded: rec });
        ins.push(LegIn { row: r.id, side, account: r.account, venue: shown(&v), spelling, holding, at, amount });
    }
    // The links: a withdrawal's to its deposit, whichever of the two names the other.
    let ids: HashSet<i64> = rows.iter().map(|r| r.id).collect();
    let kind_of: HashMap<i64, Kind> = rows.iter().map(|r| (r.id, r.kind)).collect();
    let mut links: Vec<Link> = vec![];
    for r in rows {
        let Some(to) = r.linked_to.filter(|t| ids.contains(t)) else { continue };
        let (from, to) = if r.kind.base_out() || kind_of.get(&to).is_some_and(|k| k.base_in() && !r.kind.base_in()) { (r.id, to) } else { (to, r.id) };
        let link = Link { from: (from, Side::Base), to: (to, Side::Base) };
        if !links.contains(&link) { links.push(link); }
    }
    // The balances' holdings, each read as a base leg recorded with its asset, at its venue now.
    let now = chrono::Utc::now();
    let mut balances = vec![];
    for (account, exchange, asset) in &balances_raw {
        let Some(spelling) = asset.symbol.clone() else { continue };
        let v = venue(*exchange);
        let holding = resolver::holding(&f, &resolver::Leg { venue: &v, side: Side::Base, spelling: &spelling, at: now, recorded: Some(asset) });
        balances.push(BalanceIn { account: *account, venue: shown(&v), asset_id: asset.id, spelling, holding });
    }
    // The catalogue's symbol and name of each coin and security held.
    let held: BTreeSet<String> = ins.iter().map(|l| &l.holding).chain(balances.iter().map(|b| &b.holding))
        .filter_map(|h| match h { resolver::Holding::Coin(e) | resolver::Holding::Security(e) => Some(e.clone()), _ => None }).collect();
    let mut names = Names::new();
    for (a, name) in assets_where(c, catalogue, "external_id", &held)? {
        if let Some(e) = a.external_id { names.entry(e).or_insert((a.symbol, name)); }
    }
    Ok(Spellings::build(ins, &links, &balances, &names)?)
}

/// Each row's legs take their labels from the map (a leg it has none for keeps its spelling: a blank quote).
pub fn relabel(rows: &mut [Stored], s: &Spellings) {
    for r in rows.iter_mut() {
        if let Some(l) = s.label(r.id, Side::Base) { r.base = l.clone(); }
        if let (Some(q), Some(l)) = (r.quote.as_mut(), s.label(r.id, Side::Quote)) { *q = l.clone(); }
        if let (Some(f), Some(l)) = (r.fee_currency.as_mut(), s.label(r.id, Side::Fee)) { *f = l.clone(); }
    }
    unlink(rows, &void_pairs(s));
    // A valid link whose quantity arrives under another label (unknown units at two accounts) lands under that label.
    for r in rows.iter_mut() {
        let Some(to) = r.linked_to else { continue };
        let moved = [Link { from: (r.id, Side::Base), to: (to, Side::Base) }, Link { from: (to, Side::Base), to: (r.id, Side::Base) }].iter().find_map(|l| s.transfer(l));
        r.to_base = moved.filter(|(from, arrive)| from != arrive && *from == &r.base).map(|(_, arrive)| arrive.clone());
    }
}

/// The rows of each void link, both ways round.
fn void_pairs(s: &Spellings) -> Vec<(i64, i64)> { s.void_links().iter().flat_map(|l| [(l.from.0, l.to.0), (l.to.0, l.from.0)]).collect() }

/// A link the map voids (two coins, a cut, too little held) is void for quantity too: its rows read unlinked.
fn unlink(rows: &mut [Stored], void: &[(i64, i64)]) {
    for r in rows.iter_mut().filter(|r| r.linked_to.is_some_and(|to| void.contains(&(r.id, to)))) { r.linked_to = None; r.to_venue = None; r.to_exchange_id = None; r.to_account = None; }
}

/// What reconciliation reads of the map after the walk (balances and pending moves joined to positions by label), kept
/// with each cached summary: every current balance's label, the legs whose label is not their own spelling, and each
/// such label's first raw spelling (for the lookups that still gather by spelling).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Labels { pub assets: Vec<(i64, i64, String)>, pub legs: Vec<(i64, Side, String)>, pub spellings: Vec<(String, String)>,
                    /// The void links' rows, both ways round (`Spellings::void_links`).
                    pub void: Vec<(i64, i64)>,
                    /// Each coin label's external id (the wash-sale locks take every asset of the coin).
                    pub coins: Vec<(String, String)>,
                    /// The labels of units no evidence identifies (never priced), for the ledger page.
                    pub unknown: Vec<String>,
                    /// Codex r2: a digest of every leg's and balance's resolved key, the walk's fingerprint (`backfill::fingerprint`);
                    /// not cached (a summary read back is never fingerprinted).
                    pub identity: String }

impl Labels {
    /// The table for `rows` (as read, before `relabel`) under map `s`.
    pub fn of(s: &Spellings, rows: &[Stored]) -> Labels {
        let mut assets: Vec<(i64, i64, String)> = s.asset_labels().map(|(&(a, id), l)| (a, id, l.as_str().to_string())).collect();
        assets.sort();
        let own: HashMap<(i64, Side), &str> = rows.iter().flat_map(|r| {
            [Some((Side::Base, r.base.as_str())), r.quote.as_ref().map(|q| (Side::Quote, q.as_str())), r.fee_currency.as_ref().map(|f| (Side::Fee, f.as_str()))]
                .into_iter().flatten().map(move |(side, sp)| ((r.id, side), sp))
        }).collect();
        // ponytail: every relabelled leg of the whole history; keep only legs since the earliest balance sync if the cache grows.
        let mut legs: Vec<(i64, Side, String)> = s.leg_labels().filter(|(k, l)| own.get(k).is_some_and(|sp| *sp != l.as_str())).map(|(&(r, side), l)| (r, side, l.as_str().to_string())).collect();
        legs.sort();
        let mut spellings: Vec<(String, String)> = s.leg_labels().map(|(_, l)| l).chain(s.asset_labels().map(|(_, l)| l))
            .filter_map(|l| s.raw(l).first().filter(|r| **r != l.as_str()).map(|r| (l.as_str().to_string(), r.clone()))).collect();
        spellings.sort();
        spellings.dedup();
        let mut coins: Vec<(String, String)> = s.leg_labels().map(|(_, l)| l).chain(s.asset_labels().map(|(_, l)| l))
            .filter_map(|l| l.coin().map(|ext| (l.as_str().to_string(), ext.to_string()))).collect();
        coins.sort();
        coins.dedup();
        let mut keys: Vec<String> = s.leg_labels().map(|(&(r, side), l)| format!("{r}:{side:?}:{}:{}", l.as_str(), l.key()))
            .chain(s.asset_labels().map(|(&(a, id), l)| format!("b{a}:{id}:{}:{}", l.as_str(), l.key()))).collect();
        keys.sort();
        let identity = { use sha2::Digest; hex::encode(sha2::Sha256::digest(keys.join("\n").as_bytes())) };
        let mut unknown: Vec<String> = s.leg_labels().map(|(_, l)| l).filter(|l| l.keyed() && l.class() == super::spellings::Class::Unknown).map(|l| l.as_str().to_string()).collect();
        unknown.sort();
        unknown.dedup();
        Labels { assets, legs, spellings, void: void_pairs(s), coins, unknown, identity }
    }
    /// The label of an account's balance of an asset; None when the map has none (the balance keeps its symbol).
    pub fn asset(&self, account: i64, asset: i64) -> Option<&str> { self.assets.iter().find(|(a, id, _)| *a == account && *id == asset).map(|(_, _, l)| l.as_str()) }
    /// A label's raw spelling, for a lookup by spelling; a plain label is its own.
    pub fn spelling<'a>(&'a self, label: &'a str) -> &'a str { self.spellings.iter().find(|(l, _)| l == label).map_or(label, |(_, r)| r.as_str()) }
    /// The asset a label's balance holds, if any balance has it.
    pub fn asset_of(&self, label: &str) -> Option<i64> { self.assets.iter().find(|(_, _, l)| l == label).map(|(_, id, _)| *id) }
    /// `rows` (as read) take the labels of their legs; a leg with none keeps its spelling.
    pub fn relabel(&self, rows: &mut [Stored]) {
        let of: HashMap<(i64, Side), &str> = self.legs.iter().map(|(r, side, l)| ((*r, *side), l.as_str())).collect();
        for r in rows.iter_mut() {
            if let Some(l) = of.get(&(r.id, Side::Base)) { r.base = super::spellings::Label::spelled(*l); }
            if let (Some(q), Some(l)) = (r.quote.as_mut(), of.get(&(r.id, Side::Quote))) { *q = super::spellings::Label::spelled(*l); }
            if let (Some(f), Some(l)) = (r.fee_currency.as_mut(), of.get(&(r.id, Side::Fee))) { *f = super::spellings::Label::spelled(*l); }
        }
        unlink(rows, &self.void);
    }
    /// A coin label's external id.
    pub fn coin(&self, label: &str) -> Option<&str> { self.coins.iter().find(|(l, _)| l == label).map(|(_, e)| e.as_str()) }
    /// The part of the table an account's summary reads: its balances, its rows' legs, and the spellings, coins and void
    /// pairs those name (Codex r2: one full copy per venue could push a multi-account cache past its limit).
    pub fn for_account(&self, account: i64, rows: &HashSet<i64>) -> Labels {
        let assets: Vec<(i64, i64, String)> = self.assets.iter().filter(|(a, ..)| *a == account).cloned().collect();
        let legs: Vec<(i64, Side, String)> = self.legs.iter().filter(|(r, ..)| rows.contains(r)).cloned().collect();
        // Codex r3: the per-label facts (a coin's id, an unidentified label, a raw spelling) stay whole: a leg that kept its
        // spelling is in no list of its own, and those lists are one entry per label.
        Labels { spellings: self.spellings.clone(), coins: self.coins.clone(), unknown: self.unknown.clone(),
                 void: self.void.iter().filter(|(a, b)| rows.contains(a) || rows.contains(b)).cloned().collect(),
                 assets, legs, identity: String::new() }
    }
    /// Whether the map voids the link between rows `a` and `b`.
    pub fn void(&self, a: i64, b: i64) -> bool { self.void.contains(&(a, b)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::spellings::Class;

    /// Exchanges 1 Binance, 2 KuCoin, 3 MEXC, 4 Alpaca; assets and tickers as each test lists them.
    fn db(assets: &str, tickers: &str, rows: &[&str]) -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(&format!(
            "CREATE TABLE exchanges (id INTEGER PRIMARY KEY, type TEXT);
             INSERT INTO exchanges VALUES (1, 'Exchanges::Binance'), (2, 'Exchanges::Kucoin'), (3, 'Exchanges::Mexc'), (4, 'Exchanges::Alpaca');
             CREATE TABLE exchange_account_balances (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER, account_id INTEGER, asset_id INTEGER, free NUMERIC, locked NUMERIC);
             CREATE TABLE account_transactions (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER, entry_type INTEGER, base_currency TEXT,
             base_amount NUMERIC, quote_currency TEXT, quote_amount NUMERIC, fee_currency TEXT, fee_amount NUMERIC, tx_id TEXT, group_id TEXT,
             transacted_at TEXT, raw_data TEXT, manual_values TEXT, linked_transaction_id INTEGER, base_asset_id INTEGER, transaction_id INTEGER);
             CREATE TABLE assets (id INTEGER PRIMARY KEY, external_id TEXT, symbol TEXT, name TEXT, category TEXT);
             CREATE TABLE tickers (id INTEGER PRIMARY KEY, exchange_id INTEGER, base TEXT, quote TEXT, base_asset_id INTEGER, quote_asset_id INTEGER);
             {assets} {tickers}")).unwrap();
        for values in rows {
            c.execute(&format!("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, fee_currency, \
                                fee_amount, linked_transaction_id, base_asset_id, transacted_at, raw_data, manual_values) VALUES (1, {values}, '{{}}', '{{}}')"), []).unwrap();
        }
        c
    }
    fn map(c: &Connection) -> (Vec<Stored>, Spellings) {
        crate::tracker::rows::load_all_mapped(c, 1).unwrap()
    }
    fn printed(s: &Spellings, row: i64, side: Side) -> String { s.label(row, side).unwrap().to_string() }

    const CATALOGUE: &str = "INSERT INTO assets VALUES (1, 'bitcoin', 'BTC', 'Bitcoin', 'Cryptocurrency'), (2, 'ethereum', 'ETH', 'Ethereum', 'Cryptocurrency'),
        (3, 'tether', 'USDT', 'Tether', 'Cryptocurrency'), (10, 'terra-luna', 'LUNC', 'Terra Luna Classic', 'Cryptocurrency'),
        (11, 'terra-luna-2', 'LUNA', 'Terra', 'Cryptocurrency'), (20, 'bitcoin-gold', 'BTG', 'Bitcoin Gold', NULL),
        (21, 'openverse-network', 'BTG', 'Openverse Network', 'Cryptocurrency'), (30, 'pleasure-coin', 'PLN', 'PLEARN', 'Cryptocurrency'),
        (31, 'pln', 'PLN', 'Polish Zloty', 'Fiat');";

    /// LUNA bought before the rename and LUNC sold after it are one coin (terra-luna) and net to nothing; LUNA after the
    /// rename is Terra, another label.
    #[test]
    fn luna_and_lunc_net_under_one_label() {
        let c = db(CATALOGUE, "INSERT INTO tickers VALUES (1, 1, 'LUNC', 'USDT', 10, 3), (2, 1, 'LUNA', 'USDT', 11, 3);",
                   &["1, 0, 'LUNA', 10, 'USDT', 100, NULL, NULL, NULL, NULL, '2021-06-01 00:00:00'",
                     "1, 1, 'LUNC', 10, 'USDT', 1, NULL, NULL, NULL, NULL, '2023-01-01 00:00:00'",
                     "1, 0, 'LUNA', 5, 'USDT', 5, NULL, NULL, NULL, NULL, '2023-02-01 00:00:00'"]);
        let (_, s) = map(&c);
        assert_eq!(s.label(1, Side::Base), s.label(2, Side::Base), "one coin");
        assert_eq!(printed(&s, 1, Side::Base), "LUNC", "LUNC is its own; LUNA is also Terra's");
        assert_eq!(printed(&s, 3, Side::Base), "LUNA (Terra)");
        assert_eq!(s.holding(s.label(1, Side::Base).unwrap()), Some(&resolver::Holding::Coin("terra-luna".into())));
        assert_eq!(s.class(s.label(1, Side::Quote).unwrap()), Class::Cash(resolver::CashKind::Stable));
        assert_eq!(printed(&s, 1, Side::Quote), "USDT");
    }

    /// Codex r1 (step 5): the tax reports take a label's identity from the map, whatever day the row is; an unidentified
    /// label is its own asset, never a catalogue coin of its spelling.
    #[test]
    fn tax_identity_is_the_labels() {
        use crate::tracker::prices::{self, Reference, Venue};
        let c = db(CATALOGUE, "INSERT INTO tickers VALUES (1, 1, 'LUNC', 'USDT', 10, 3), (2, 1, 'LUNA', 'USDT', 11, 3);",
                   &["1, 0, 'LUNA', 10, 'USDT', 100, NULL, NULL, NULL, NULL, '2021-06-01 00:00:00'",
                     "1, 1, 'LUNC', 10, 'USDT', 1, NULL, NULL, NULL, NULL, '2023-01-01 00:00:00'",
                     "1, 0, 'LUNA', 5, 'USDT', 5, NULL, NULL, NULL, NULL, '2023-02-01 00:00:00'",
                     "3, 0, 'ZZZ', 1, 'USDT', 1, NULL, NULL, NULL, NULL, '2024-01-01 00:00:00'"]);
        c.execute_batch("ALTER TABLE assets ADD COLUMN market_cap_rank INTEGER;
                         CREATE TABLE historical_prices (id INTEGER PRIMARY KEY, asset TEXT, currency TEXT, date TEXT, price NUMERIC, UNIQUE (asset, currency, date));").unwrap();
        let (rows, _) = map(&c);
        let mut r = Reference::load(&c, &Venue::alpaca(&c).unwrap(), &prices::symbols(&rows)).unwrap();
        r.learn(&c, &rows).unwrap();
        let day = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok();
        let binance = r.venue("binance");
        for on in ["2021-06-01", "2023-01-01"] { assert_eq!(prices::tax_identity(&r, "LUNC", binance, day(on)), "terra-luna", "{on}"); }
        assert_eq!(prices::tax_identity(&r, "LUNA (Terra)", binance, day("2023-02-01")), "terra-luna-2");
        assert_eq!(prices::tax_identity(&r, "ZZZ", r.venue("mexc"), day("2024-01-01")), "unknown:ZZZ");
        // Codex r2: by the label, an unidentified unit keeps its account (two accounts may print it alike).
        let zzz = rows.iter().find(|x| x.base.as_str() == "ZZZ").unwrap();
        assert_eq!(prices::tax_identity_of(&r, &zzz.base, r.venue("mexc"), day("2024-01-01")), "unknown:-3:ZZZ");
        // The Kursliste names a coin by its catalogue symbol, not by a decorated label.
        assert_eq!((r.official("LUNA (Terra)"), r.official("LUNC"), r.official("ZZZ")), ("LUNA", "LUNC", "ZZZ"));
    }

    /// Codex r1: a security the map resolved (Alpaca's BTC ETF beside bitcoin, both recorded) resolves as a security, so
    /// the backfill does not classify its symbol again (both classes recorded under BTC: no guess, no closes).
    #[test]
    fn a_resolved_security_is_a_security() {
        use crate::tracker::prices::{self, Reference, Resolved, Venue};
        let c = db(&format!("{CATALOGUE} INSERT INTO assets VALUES (40, 'btc-etf', 'BTC', 'Bitcoin ETF', 'ETF');"), "",
                   &["4, 0, 'BTC', 1, 'USD', 40, NULL, NULL, NULL, 40, '2026-09-01 00:00:00'",
                     "4, 0, 'BTC', 1, 'USD', 60000, NULL, NULL, NULL, 1, '2026-09-02 00:00:00'"]);
        c.execute_batch("ALTER TABLE assets ADD COLUMN market_cap_rank INTEGER;
                         CREATE TABLE historical_prices (id INTEGER PRIMARY KEY, asset TEXT, currency TEXT, date TEXT, price NUMERIC, UNIQUE (asset, currency, date));").unwrap();
        let (rows, _) = map(&c);
        let (etf, coin) = (rows[0].base.to_string(), rows[1].base.to_string());
        let mut r = Reference::load(&c, &Venue::alpaca(&c).unwrap(), &prices::symbols(&rows)).unwrap();
        r.learn(&c, &rows).unwrap();
        assert!(matches!(r.resolve(&etf), Resolved::Security), "{etf}");
        assert!(matches!(r.resolve(&coin), Resolved::Coin(ref e) if e == "bitcoin"), "{coin}");
    }

    /// Binance's BTG is Bitcoin Gold (the alias), KuCoin's is what KuCoin lists: two labels; an unrecorded KuCoin BTG
    /// with no listing would be unclear (two catalogue candidates).
    #[test]
    fn btg_on_binance_and_kucoin_are_two_coins() {
        let c = db(CATALOGUE, "INSERT INTO tickers VALUES (1, 2, 'BTG', 'USDT', 21, 3);",
                   &["1, 0, 'BTG', 1, 'USDT', 20, NULL, NULL, NULL, NULL, '2018-01-01 00:00:00'",
                     "2, 0, 'BTG', 100, 'USDT', 1, NULL, NULL, NULL, NULL, '2025-01-01 00:00:00'"]);
        let (_, s) = map(&c);
        assert_eq!(printed(&s, 1, Side::Base), "BTG (Bitcoin Gold)");
        assert_eq!(printed(&s, 2, Side::Base), "BTG (Openverse Network)");
    }

    /// A quote leg is the coin it names: MEXC's ETH bought for BTC spends the same bitcoin a Binance BTC buy books.
    #[test]
    fn a_quote_leg_is_its_coin() {
        let c = db(CATALOGUE, "INSERT INTO tickers VALUES (1, 3, 'ETH', 'BTC', 2, 1), (2, 1, 'BTC', 'USDT', 1, 3);",
                   &["1, 0, 'BTC', 1, 'USDT', 30000, NULL, NULL, NULL, NULL, '2024-01-01 00:00:00'",
                     "3, 0, 'ETH', 10, 'BTC', 0.5, 'BTC', 0.001, NULL, NULL, '2024-02-01 00:00:00'"]);
        let (_, s) = map(&c);
        assert_eq!(s.label(2, Side::Quote), s.label(1, Side::Base));
        assert_eq!(s.label(2, Side::Fee), s.label(1, Side::Base), "a fee in the quote coin");
        assert_eq!(printed(&s, 2, Side::Quote), "BTC");
    }

    /// The dirty fiat pair: MEXC's base PLN is PLEARN, a złoty quote is cash, and they print apart.
    #[test]
    fn plearn_and_the_zloty_print_apart() {
        let c = db(CATALOGUE, "INSERT INTO tickers VALUES (1, 3, 'PLN', 'USDT', 30, 3);",
                   &["3, 0, 'PLN', 1000, 'USDT', 10, NULL, NULL, NULL, 30, '2024-01-01 00:00:00'",
                     "1, 0, 'BTC', 1, 'PLN', 100000, NULL, NULL, NULL, NULL, '2024-01-02 00:00:00'"]);
        let (_, s) = map(&c);
        assert_eq!(printed(&s, 1, Side::Base), "PLN (PLEARN)");
        assert_eq!(printed(&s, 2, Side::Quote), "PLN");
        assert_eq!(s.class(s.label(2, Side::Quote).unwrap()), Class::Cash(resolver::CashKind::Fiat));
        // The label answers alone, from its identity.
        let (plearn, zloty) = (s.label(1, Side::Base).unwrap(), s.label(2, Side::Quote).unwrap());
        assert_eq!((plearn.class(), plearn.coin(), plearn.currency()), (Class::Coin, Some("pleasure-coin"), None));
        assert_eq!((zloty.is_fiat(), zloty.is_stable(), zloty.currency()), (true, false, Some("PLN")));
        assert!(s.label(1, Side::Quote).unwrap().is_stable());
    }

    /// Balances are an input (a balance of another coin under a history's spelling splits it), and an unidentified
    /// spelling is per account: two venues' unlisted `ZZZ` are two labels.
    #[test]
    fn balances_split_a_spelling_and_unknowns_stay_per_account() {
        let c = db(CATALOGUE, "INSERT INTO tickers VALUES (1, 1, 'BTG', 'USDT', 20, 3);",
                   &["1, 0, 'BTG', 1, 'USDT', 20, NULL, NULL, NULL, NULL, '2019-01-01 00:00:00'",
                     "1, 0, 'ZZZ', 1, 'USDT', 1, NULL, NULL, NULL, NULL, '2024-01-01 00:00:00'",
                     "3, 0, 'ZZZ', 1, 'USDT', 1, NULL, NULL, NULL, NULL, '2024-01-01 00:00:00'"]);
        let (_, s) = map(&c);
        assert_eq!(printed(&s, 1, Side::Base), "BTG");
        assert_ne!(s.label(2, Side::Base), s.label(3, Side::Base));
        assert_eq!((printed(&s, 2, Side::Base), printed(&s, 3, Side::Base)), ("ZZZ".into(), "ZZZ (unidentified on mexc)".into()), "one spelling on two venues prints apart");
        c.execute("INSERT INTO exchange_account_balances (user_id, exchange_id, account_id, asset_id, free, locked) VALUES (1, 2, NULL, 21, 5, 0)", []).unwrap();
        c.execute("INSERT INTO tickers VALUES (2, 2, 'BTG', 'USDT', 21, 3)", []).unwrap();
        let (_, s) = map(&c);
        assert_eq!(printed(&s, 1, Side::Base), "BTG (Bitcoin Gold)");
        assert_eq!(s.label_of_asset(-2, 21).unwrap().to_string(), "BTG (Openverse Network)");
    }

    /// Codex r1: two coins held as BTG (Bitcoin Gold on Binance, Openverse on KuCoin) reconcile apart. By symbol, the one
    /// `BTG` balance group matches neither position: Bitcoin Gold's lot reads as departed and all six units as arrived.
    #[test]
    fn balances_and_pending_rows_reconcile_by_label() {
        use crate::tracker::{figures::Balance, walk::{Position, Summary}};
        let c = db(CATALOGUE, "INSERT INTO tickers VALUES (1, 1, 'BTG', 'USDT', 20, 3); INSERT INTO tickers VALUES (2, 2, 'BTG', 'USDT', 21, 3);",
                   &["1, 0, 'BTG', 1, 'USDT', 20, NULL, NULL, NULL, NULL, '2019-01-01 00:00:00'"]);
        c.execute_batch("INSERT INTO exchange_account_balances (user_id, exchange_id, account_id, asset_id, free, locked) VALUES (1, 1, NULL, 20, 1, 0), (1, 2, NULL, 21, 5, 0)").unwrap();
        let (mut rows, labels) = crate::tracker::rows::load_all_labelled(&c, 1).unwrap();
        assert_eq!(labels.asset(-1, 20), Some("BTG (Bitcoin Gold)"));
        assert_eq!(labels.asset(-2, 21), Some("BTG (Openverse Network)"));
        assert_eq!(labels.spelling("BTG (Openverse Network)"), "BTG");
        // A pending row, read by its spelling, takes its leg's label.
        rows[0].base = crate::tracker::spellings::Label::spelled("BTG");
        labels.relabel(&mut rows);
        assert_eq!(rows[0].base.as_str(), "BTG (Bitcoin Gold)");
        let d = |s: &str| Dec::strict(s).unwrap();
        let balance = |account: i64, asset: i64, units: &str| Balance { asset, account, category: Some("Cryptocurrency".into()), symbol: "BTG".into(), exchange: "x".into(),
            free: d(units), locked: Dec::zero(), usd_price: Some(d("10")), usd_value: Some((&d(units) * &d("10")).unwrap()), priced_at: None, synced_at: None };
        let mut summary = Summary::empty();
        summary.positions = vec![Position { symbol: "BTG (Bitcoin Gold)".into(), quantity: d("1"), cost: d("20"), avg_cost: d("20"), opened_at: None, estimated: false, unpriced: Dec::zero() }];
        let balances = [balance(-1, 20, "1"), balance(-2, 21, "5")];
        let notes = |s: &Summary| crate::tracker::gaps::resolve(s, &balances, "x").unwrap().into_iter().map(|n| (n.kind, n.symbol)).collect::<Vec<_>>();
        assert_eq!(notes(&summary), [("arrived", "BTG".to_string()), ("left", "BTG (Bitcoin Gold)".to_string())], "by symbol");
        summary.labels = labels;
        assert_eq!(notes(&summary), [("arrived", "BTG (Openverse Network)".to_string())], "by label: only the Openverse units arrived");
    }

    /// Codex r1: a valid link between unknown units at two accounts that print apart lands its lots under the
    /// deposit's label, so the destination's own rows find them.
    #[test]
    fn a_valid_link_lands_under_the_deposits_label() {
        let c = db(CATALOGUE, "",
                   &["1, 0, 'ZZZ', 1, 'USDT', 1, NULL, NULL, NULL, NULL, '2024-01-01 00:00:00'",
                     "1, 5, 'ZZZ', 1, NULL, NULL, NULL, NULL, 3, NULL, '2024-01-02 00:00:00'",
                     "3, 4, 'ZZZ', 1, NULL, NULL, NULL, NULL, NULL, NULL, '2024-01-02 01:00:00'"]);
        let (rows, s) = map(&c);
        assert!(s.void_links().is_empty());
        let sent = rows.iter().find(|r| r.id == 2).unwrap();
        assert_eq!((sent.base.as_str(), sent.linked_to), ("ZZZ", Some(3)));
        assert_eq!(sent.to_base.as_ref().map(|l| l.as_str()), Some("ZZZ (unidentified on mexc)"));
        assert_eq!(rows.iter().find(|r| r.id == 3).unwrap().base.as_str(), "ZZZ (unidentified on mexc)");
        assert_eq!(rows.iter().find(|r| r.id == 1).unwrap().to_base, None, "no link, no arrival label");
    }

    /// Codex r1: a link the map voids (Bitcoin Gold out of Binance, Openverse into KuCoin, both BTG) is void for quantity
    /// too: the walk, the backfill and the tax read both rows unlinked, and a pending move does not net them.
    #[test]
    fn a_void_link_reads_unlinked() {
        let c = db(CATALOGUE, "INSERT INTO tickers VALUES (1, 1, 'BTG', 'USDT', 20, 3); INSERT INTO tickers VALUES (2, 2, 'BTG', 'USDT', 21, 3);",
                   &["1, 0, 'BTG', 1, 'USDT', 20, NULL, NULL, NULL, NULL, '2019-01-01 00:00:00'",
                     "1, 5, 'BTG', 1, NULL, NULL, NULL, NULL, 3, NULL, '2019-01-02 00:00:00'",
                     "2, 4, 'BTG', 1, NULL, NULL, NULL, NULL, NULL, NULL, '2019-01-02 01:00:00'"]);
        let (rows, s) = map(&c);
        assert_eq!(s.void_links(), &[Link { from: (2, Side::Base), to: (3, Side::Base) }]);
        let sent = rows.iter().find(|r| r.id == 2).unwrap();
        assert_eq!((sent.linked_to, sent.to_account), (None, None), "void for quantity");
        assert_eq!(sent.base.as_str(), "BTG (Bitcoin Gold)");
        assert_eq!(rows.iter().find(|r| r.id == 3).unwrap().base.as_str(), "BTG (Openverse Network)");
        let (_, labels) = crate::tracker::rows::load_all_labelled(&c, 1).unwrap();
        assert!(labels.void(2, 3) && labels.void(3, 2) && !labels.void(1, 2));
    }

    /// An account's summary keeps only its part of the label table: its balances, its rows' legs, what those name.
    #[test]
    fn an_accounts_part_of_the_labels_is_its_own() {
        let l = Labels { assets: vec![(-1, 20, "BTG (Bitcoin Gold)".into()), (-2, 21, "BTG (Openverse Network)".into())],
                         legs: vec![(1, Side::Base, "BTG (Bitcoin Gold)".into()), (2, Side::Base, "BTG (Openverse Network)".into())],
                         spellings: vec![("BTG (Bitcoin Gold)".into(), "BTG".into()), ("BTG (Openverse Network)".into(), "BTG".into())],
                         void: vec![(1, 3), (3, 1)], coins: vec![("BTG (Bitcoin Gold)".into(), "bitcoin-gold".into()), ("BTG (Openverse Network)".into(), "openverse-network".into())],
                         unknown: vec![], identity: "x".into() };
        let mine = l.for_account(-1, &HashSet::from([1]));
        assert_eq!((mine.assets.len(), mine.legs.len(), mine.void.len()), (1, 1, 2));
        assert_eq!((mine.coin("BTG (Bitcoin Gold)"), mine.spellings.len()), (Some("bitcoin-gold"), 2), "per-label facts stay whole");
    }

    /// Codex r2: the ledger reads a row's quote as cash by its label: a venue's coin spelled `PLN` (PLEARN on MEXC) is no
    /// złoty, so the row is not priced by its counterpart as if it were.
    #[test]
    fn a_coin_quote_spelled_like_cash_is_no_counterpart() {
        let c = db(CATALOGUE, "INSERT INTO tickers VALUES (1, 3, 'BTC', 'PLN', 1, 30);",
                   &["3, 0, 'BTC', 1, 'PLN', 500, NULL, NULL, NULL, NULL, '2024-01-01 00:00:00'"]);
        let (_, s) = map(&c);
        assert_eq!(printed(&s, 1, Side::Quote), "PLN (PLEARN)");
        let row = crate::web::tracker::row::Row { id: 1, owner: 1, exchange: 3, kind: 0, base: "BTC".into(), amount: Dec::one(), quote: Some("PLN".into()),
            quoted: Some(Dec::from_i64(500)), fee: None, fee_currency: None, at: chrono::Utc::now(), group: None, manual: serde_json::json!({}), manual_null: false,
            linked: None, inverse: None, asset: None, bot: None, venue: "mexc".into(), raw: serde_json::json!({}) };
        assert!(row.counterpart(&c, None).unwrap().is_some(), "by its spelling, PLN is the złoty");
        assert!(row.counterpart(&c, Some(&s)).unwrap().is_none(), "by its label, PLEARN");
    }

    /// A stored link carries a known withdrawal's coin to an unlisted deposit, whichever row names the other.
    #[test]
    fn a_link_carries_the_coin_either_way_it_is_stored() {
        for (w_link, d_link) in [("2", "NULL"), ("NULL", "1")] {
            let c = db(CATALOGUE, "INSERT INTO tickers VALUES (1, 1, 'BTG', 'USDT', 20, 3);",
                       &[&format!("1, 5, 'BTG', 1, NULL, NULL, NULL, NULL, {w_link}, NULL, '2019-01-01 00:00:00'"),
                         &format!("3, 4, 'BTG', 1, NULL, NULL, NULL, NULL, {d_link}, NULL, '2019-01-01 01:00:00'")]);
            let (_, s) = map(&c);
            assert_eq!(s.label(2, Side::Base), s.label(1, Side::Base), "linked {w_link}/{d_link}");
            assert!(s.void_links().is_empty());
        }
    }

    /// Plan r10 PR1 step 3: a coin label is priced by its coin, from `asset_prices`; a bare legacy row under its spelling
    /// (BTG: main's rank guess) and a legacy `coin:` text row are no coin's prices; an unidentified label has no price; a
    /// fetched price is stored by the coin; the generation moves with a write to either table.
    #[test]
    fn a_coin_is_priced_by_its_identity() {
        use crate::tracker::prices::{self, PriceService, Reference, Venue};
        let c = db(CATALOGUE, "", &["1, 0, 'BTG', 1, 'USDT', 20, NULL, NULL, NULL, NULL, '2019-01-01 00:00:00'",
                                    "1, 0, 'ZZZ', 1, 'USDT', 1, NULL, NULL, NULL, NULL, '2019-01-01 00:00:00'"]);
        c.execute_batch("ALTER TABLE assets ADD COLUMN market_cap_rank INTEGER;
                         CREATE TABLE historical_prices (id INTEGER PRIMARY KEY, asset TEXT, currency TEXT, date TEXT, price NUMERIC, UNIQUE (asset, currency, date));
                         CREATE TABLE asset_prices (id INTEGER PRIMARY KEY, external_id TEXT NOT NULL, currency TEXT NOT NULL, date TEXT NOT NULL, price TEXT NOT NULL, UNIQUE (external_id, currency, date));
                         CREATE TABLE fx_rates (currency TEXT, date TEXT, rate NUMERIC);
                         INSERT INTO historical_prices (asset, currency, date, price) VALUES ('BTG', 'USD', '2019-01-01', 0.5), ('coin:bitcoin', 'USD', '2019-01-01', 1);
                         INSERT INTO asset_prices (external_id, currency, date, price) VALUES ('bitcoin-gold', 'USD', '2019-01-01', '20');").unwrap();
        let before = crate::tracker::backfill::generation(&c).unwrap();
        let (rows, _) = crate::tracker::rows::load_all_mapped(&c, 1).unwrap();
        let mut r = Reference::load(&c, &Venue::alpaca(&c).unwrap(), &prices::symbols(&rows)).unwrap();
        r.learn(&c, &rows).unwrap();
        let fx = crate::tracker::fx::FxTable::load(&c).unwrap();
        let mut none = prices::Unfetched;
        let mut ps = PriceService::new(&r, &fx, chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(), &mut none);
        let at = crate::figures::at::At::from_sql("2019-01-01 12:00:00").unwrap();
        let btg = rows[0].base.to_string();
        assert_eq!(btg, "BTG");
        assert_eq!(ps.price_at(&btg, "USD", at, Some("binance")).unwrap().to_s_f(), "20.0", "Bitcoin Gold's price, not the bare BTG row");
        assert!(ps.price_at("ZZZ", "USD", at, Some("binance")).unwrap().is_zero(), "unidentified: no price");
        assert!(ps.warnings.iter().any(|w| w.starts_with("ZZZ/USD")));
        assert!(ps.price_at(&prices::coin_key("bitcoin"), "USD", at, None).unwrap().is_zero(), "a legacy coin: text row is no price");
        // Stored by the coin, and the generation moves.
        prices::store(&c, &[(prices::coin_key("bitcoin"), chrono::NaiveDate::from_ymd_opt(2019, 1, 1).unwrap(), "3500".into())]).unwrap();
        let n: i64 = c.query_row("SELECT count(*) FROM asset_prices WHERE external_id = 'bitcoin'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        assert!(crate::tracker::backfill::generation(&c).unwrap() > before);
        assert_eq!(prices::stored_days(&c, &prices::coin_key("bitcoin"), "USD", chrono::NaiveDate::from_ymd_opt(2019, 1, 1).unwrap(), chrono::NaiveDate::from_ymd_opt(2019, 1, 2).unwrap()).unwrap().len(), 1);
    }
}
