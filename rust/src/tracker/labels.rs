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
fn assets_where(c: &Connection, column: &str, values: &BTreeSet<String>) -> Result<Vec<(AssetRef, Option<String>)>, FiguresError> {
    let mut out = vec![];
    let values: Vec<&String> = values.iter().collect();
    for chunk in values.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(", ");
        let mut s = c.prepare(&format!("SELECT id, external_id, symbol, category, name FROM assets WHERE {column} IN ({marks}) ORDER BY id"))?;
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

/// The owner's map over `rows` (every row `load_all` reads, borrowed wallets' included).
pub fn load(c: &Connection, user_id: i64, rows: &[Stored]) -> Result<Spellings, FiguresError> {
    // The venues: the rows' and the balances'.
    let mut s = c.prepare("SELECT id, type FROM exchanges ORDER BY id")?;
    let kinds: HashMap<i64, Option<String>> = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    let venue = |id: i64| {
        let t = kinds.get(&id).cloned().flatten().unwrap_or_default();
        VenueRef { id, name_id: super::rows::name_id(&t), broker: super::prices::STOCK_TYPES.contains(&t.as_str()) }
    };
    // The balances, by account (as the rows' account: the membership's, else minus the exchange id).
    let has_account = c.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('exchange_account_balances') WHERE name = 'account_id')", [], |r| r.get::<_, bool>(0))?;
    let (account, counted) = if has_account {
        ("COALESCE(b.account_id, -b.exchange_id)".to_string(), crate::identity::counted(c, user_id, "b.account_id")?)
    } else { ("-b.exchange_id".to_string(), "1".to_string()) };
    let mut s = c.prepare(&format!("SELECT {account}, b.exchange_id, a.id, a.external_id, a.symbol, a.category FROM exchange_account_balances b \
                                    JOIN assets a ON a.id = b.asset_id WHERE b.user_id = ?1 AND {counted} AND b.free + b.locked > 0 ORDER BY b.id"))?;
    let mut balances_raw: Vec<(i64, i64, AssetRef)> = vec![];
    let mut q = s.query([user_id])?;
    while let Some(r) = q.next()? {
        charge(1, 0)?;
        balances_raw.push((r.get(0)?, r.get(1)?, AssetRef { id: r.get(2)?, external_id: r.get(3)?, symbol: r.get(4)?, category: r.get(5)? }));
    }
    let venues: BTreeSet<i64> = rows.iter().map(|r| r.exchange_id).chain(balances_raw.iter().map(|b| b.1)).collect();
    // The facts: each venue's live listings by side, its tombstoned bases, the catalogue under the owner's spellings.
    let mut f = Facts::default();
    for chunk in venues.iter().collect::<Vec<_>>().chunks(500) {
        let marks = vec!["?"; chunk.len()].join(", ");
        let mut s = c.prepare(&format!(
            "SELECT t.exchange_id, t.base, t.quote, b.id, b.external_id, b.symbol, b.category, q.id, q.external_id, q.symbol, q.category FROM tickers t \
             LEFT JOIN assets b ON b.id = t.base_asset_id LEFT JOIN assets q ON q.id = t.quote_asset_id WHERE t.exchange_id IN ({marks}) ORDER BY t.id"))?;
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
    for (a, _) in assets_where(c, "symbol", &spellings)? {
        if let Some(symbol) = a.symbol.clone() { f.catalogue.entry(symbol).or_default().push(a); }
    }
    let recorded_ids: BTreeSet<String> = rows.iter().filter_map(|r| r.base_asset.as_ref().map(|a| a.0.to_string())).collect();
    let recorded: HashMap<i64, AssetRef> = assets_where(c, "id", &recorded_ids)?.into_iter().map(|(a, _)| (a.id, a)).collect();
    // Each leg's holding.
    let mut ins = Vec::with_capacity(legs.len());
    for (i, side, spelling, amount) in legs {
        charge(1, 0)?;
        let r = &rows[i];
        let v = venue(r.exchange_id);
        let at = r.at.utc();
        let rec = (side == Side::Base).then(|| r.base_asset.as_ref().and_then(|a| recorded.get(&a.0))).flatten();
        let holding = resolver::holding(&f, &resolver::Leg { venue: &v, side, spelling: &spelling, at, recorded: rec });
        ins.push(LegIn { row: r.id, side, account: r.account, venue: v.name_id.clone(), spelling, holding, at, amount });
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
        balances.push(BalanceIn { account: *account, venue: v.name_id.clone(), asset_id: asset.id, spelling, holding });
    }
    // The catalogue's symbol and name of each coin and security held.
    let held: BTreeSet<String> = ins.iter().map(|l| &l.holding).chain(balances.iter().map(|b| &b.holding))
        .filter_map(|h| match h { resolver::Holding::Coin(e) | resolver::Holding::Security(e) => Some(e.clone()), _ => None }).collect();
    let mut names = Names::new();
    for (a, name) in assets_where(c, "external_id", &held)? {
        if let Some(e) = a.external_id { names.entry(e).or_insert((a.symbol, name)); }
    }
    Ok(Spellings::build(ins, &links, &balances, &names)?)
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
        let rows = crate::tracker::rows::load_all(c, 1).unwrap();
        let s = load(c, 1, &rows).unwrap();
        (rows, s)
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
}
