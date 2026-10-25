//! Kraken's syncs (plan Kraken sync). Slice K1: `KrakenIdentity`, which asset a Kraken asset code names (§2.4, D-KR-2,
//! D-KR-3), as the balance sync reads BalanceEx. The ledger walk and its normaliser come with K2.
use super::ledger::{only, rule_venue, spellings};
use super::SyncError;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

/// Kraken's legacy X-codes that Exchanges::Kraken::ASSET_MAP lacks (D-KR-2): Rails drops a balance under any of them.
const LEGACY: [(&str, &str); 8] = [("XLTC", "LTC"), ("XXRP", "XRP"), ("XXLM", "XLM"), ("XXMR", "XMR"), ("XETC", "ETC"), ("XZEC", "ZEC"),
                                   ("XMLN", "MLN"), ("XREP", "REP")];
/// The terminal suffixes of a code that holds the same asset elsewhere in the account: staked (.S), opt-in rewards (.M),
/// parachain (.P), yield-bearing (.B), Kraken Rewards flexible (.F). One is stripped, never more.
// ponytail: `.P` is not in Kraken's docs, only seen in the wild; drop it if a live sample shows another meaning.
const SUFFIXES: [&str; 5] = [".S", ".M", ".P", ".B", ".F"];

/// Which asset a Kraken code names, over every listing of the venue (unavailable and tombstoned included, D-KR-6). Each
/// step answers only when exactly one asset fits; two candidates or none fall to the next step, and after the last to None.
/// 1. The code itself (Tax::AssetIdentity's dated alias first, then the listings' spellings).
/// 2. ASSET_MAP and the legacy X-codes, then step 1.
/// 3. One terminal suffix stripped, then steps 1–2.
/// 4. A `.T` code: the tokenized resolver only, never the coin (D-KR-3).
/// 5. The tokenized resolver on the code (NVDASPV is NVDAx).
/// 6. An asset listed on Kraken whose symbol is the code (the canonical symbol a CSV row carries, r3).
pub struct KrakenIdentity {
    venue: crate::tracker::prices::Venue,
    spellings: HashMap<String, Vec<i64>>,
    /// The assets marked as tokenized wrappers (Asset.mark_tokenized!).
    tokenized: HashSet<i64>,
    /// The listed assets by their own symbol.
    symbols: HashMap<String, Vec<i64>>,
    /// The aliases' coins by external id.
    aliases: HashMap<String, i64>,
}

impl KrakenIdentity {
    pub fn load(c: &Connection, exchange_id: i64) -> Result<Self, SyncError> {
        let listed = "SELECT DISTINCT a.id, a.symbol, a.instrument_type FROM assets a JOIN tickers t ON a.id IN (t.base_asset_id, t.quote_asset_id) WHERE t.exchange_id = ?1 ORDER BY a.id";
        let mut s = c.prepare(listed)?;
        let rows = s.query_map([exchange_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
        let (mut tokenized, mut symbols) = (HashSet::new(), HashMap::<String, Vec<i64>>::new());
        for (id, symbol, kind) in rows {
            if kind.as_deref() == Some("tokenized") { tokenized.insert(id); }
            if let Some(symbol) = symbol { symbols.entry(symbol).or_default().push(id); }
        }
        let mut aliases = HashMap::new();
        let mut s = c.prepare("SELECT id FROM assets WHERE external_id = ?1 LIMIT 1")?;
        for coin in crate::tracker::prices::alias_coins() {
            if let Some(id) = rusqlite::OptionalExtension::optional(s.query_row([coin], |r| r.get::<_, i64>(0)))? { aliases.insert(coin.to_string(), id); }
        }
        Ok(Self { venue: rule_venue(exchange_id, "Exchanges::Kraken"), spellings: spellings(c, exchange_id)?, tokenized, symbols, aliases })
    }

    /// The asset `code` names, if exactly one does.
    pub fn asset_id(&self, code: &str) -> Option<i64> {
        if let Some(head) = code.strip_suffix(".T") { return self.tokenized(head); }
        self.mapped(code)
            .or_else(|| SUFFIXES.iter().find_map(|s| code.strip_suffix(s)).and_then(|head| self.mapped(head)))
            .or_else(|| self.tokenized(code))
            .or_else(|| only(self.symbols.get(code)))
    }

    fn mapped(&self, code: &str) -> Option<i64> {
        let map = crate::venue::kraken::ASSET_MAP.iter().chain(LEGACY.iter()).find(|(k, _)| *k == code).map(|(_, v)| *v);
        self.plain(code).or_else(|| map.and_then(|name| self.plain(name)))
    }

    /// AssetRule::Crypto's answer for one name: a dated alias names its coin, else the one listing spelt so.
    fn plain(&self, name: &str) -> Option<i64> {
        // Balances are read now: every dated alias has lapsed, so none is asked about a date.
        if let Some(coin) = crate::tracker::prices::alias_coin(name, Some(&self.venue), None) { return self.aliases.get(&coin).copied(); }
        only(self.spellings.get(&name.to_uppercase()))
    }

    /// Exchanges::Kraken#tokenized_asset_from_symbol: the SPV code spelt as its x form, upcased, among tokenized wrappers only.
    fn tokenized(&self, code: &str) -> Option<i64> {
        let name = code.strip_suffix("SPV").map_or_else(|| code.to_string(), |head| format!("{head}x")).to_uppercase();
        let ids: Vec<i64> = self.spellings.get(&name)?.iter().copied().filter(|id| self.tokenized.contains(id)).collect();
        only(Some(&ids))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> KrakenIdentity {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE assets(id INTEGER PRIMARY KEY, external_id TEXT, symbol TEXT, instrument_type TEXT);
            CREATE TABLE tickers(id INTEGER PRIMARY KEY, exchange_id INTEGER, base TEXT, quote TEXT, base_asset_id INTEGER, quote_asset_id INTEGER);
            INSERT INTO assets VALUES (1,'usd','USD',NULL),(2,'bitcoin','BTC',NULL),(3,'litecoin','LTC',NULL),(4,'polkadot','DOT',NULL),
              (5,'nvidia-xstock','NVDAx','tokenized'),(6,'solana','SOL',NULL),(7,'matic-network','MATIC',NULL),(8,'twin-a','TWIN',NULL),(9,'twin-b','TWIN',NULL),
              (10,'terra-luna-2','LUNA',NULL),(11,'brk-xstock','BRK.Bx','tokenized');
            INSERT INTO tickers VALUES (1,1,'XBT','USD',2,1),(2,1,'LTC','USD',3,1),(3,1,'DOT','USD',4,1),(4,1,'NVDAx','USD',5,1),(5,1,'SOL','USD',6,1),
              (6,1,'POL','USD',7,1),(7,1,'TWINA','USD',8,1),(8,1,'TWINB','USD',9,1),(9,1,'__stale_3_LUNA','USD',10,1),(10,1,'BRK.Bx','USD',11,1);").unwrap();
        KrakenIdentity::load(&c, 1).unwrap()
    }

    #[test]
    fn every_step_of_the_identity() {
        let k = identity();
        for (code, want) in [("XBT", Some(2)), ("XXBT", Some(2)), ("ZUSD", Some(1)), ("USD", Some(1)), ("XLTC", Some(3)), ("DOT.S", Some(4)), ("DOT.F", Some(4)),
                             ("DOT.P", Some(4)), ("XLTC.S", Some(3)), ("DOT.S.S", None), ("NVDAx", Some(5)), ("NVDASPV", Some(5)), ("NVDAx.T", Some(5)),
                             ("SOL.T", None), ("BRK.Bx", Some(11)), ("MATIC", Some(7)), ("LUNA", Some(10)), ("BTC", Some(2)), ("TWIN", None), ("ZZZ", None)] {
            assert_eq!(k.asset_id(code), want, "{code}");
        }
    }
}
