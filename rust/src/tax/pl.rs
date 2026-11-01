//! Poland: a PIT-38 part E workpaper, not a per-sale FIFO (Rails' file). The law (specs/law-PL.md):
//! - D-TAX-PL-COST-POOL. Revenue is the cash a coin is disposed of for, gross (art. 17 ust. 1 pkt 11, ust. 1f); costs are
//!   the cash spent acquiring coins and the fees of such trades, deducted in the year paid (art. 22 ust. 14–15); a surplus
//!   of costs joins the next year's (ust. 16); income b − (c + d) at 19% (art. 30b ust. 1a–1b), never a loss. No lot is
//!   matched to a sale. A coin↔coin swap (a stablecoin too: D-TAX-PL-STABLECOIN, the flag is not read) is no disposal and
//!   its costs are none (art. 23 ust. 1 pkt 38d). A fee paid in a coin settles an obligation in virtual currency (art. 17
//!   ust. 1f): revenue at its dollar value, and on a trade for cash a cost too (art. 22 ust. 14); on a swap no cost
//!   (D-TAX-PL-SWAP-FEE, the statutory reading, no ruling found).
//! - D-TAX-PL-NBP. Every value in its own currency and on its own day in the user's zone, at NBP's table A mid rate of the
//!   last table before that day (art. 11a, `nbp::Nbp::before`); a missing rate is a missing value, valued 0.
//! - D-TAX-PL-REWARDS. Staking, airdrops, mining and lending paid in coins are neither revenue nor cost; listed.
//!
//! Each value is rounded to the grosz, the columns summed from those; the base to whole złoty, then 19%, then whole złoty
//! (Ordynacja podatkowa art. 63: half up). Column d is reconstructed from the synced history (the year table shows it),
//! unless the user declared it for a year (D-TAX-PL-DECLARED-CARRY): that year's d is the declared amount (for 2019 the
//! costs before 2019 never deducted, art. 23 of the 2018 act; later, column f of the previous return), and later years
//! chain from it.
//! Read from the report's copy of the rows (D-TAX-COIN-QUOTE applies), never the ECB-enriched values.
use super::*;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side { Revenue, Cost }

const REVENUE: &[Side] = &[Side::Revenue];
const COST: &[Side] = &[Side::Cost];
const BOTH: &[Side] = &[Side::Revenue, Side::Cost];

/// What a value is, before conversion: cash in its currency, or a fee paid in a coin (its amount).
enum Money { Cash(Dec, String), CoinFee(Dec) }

/// One value of a row: the coin it concerns (and how much of it), on the row's local day, as revenue, cost or both.
struct Component<'a> { row: &'a Stored, day: NaiveDate, asset: String, amount: Option<Dec>, sides: &'static [Side], money: Money }

/// A component converted: the amount and currency converted, the rate and its table's date (None for PLN), the grosz.
struct Valued<'a> { c: Component<'a>, original: Dec, currency: String, rate: Option<(Option<NaiveDate>, Dec)>, value: Dec, missing: bool }

/// A group's fiat legs and coin legs.
type Legs<'a> = (Vec<&'a Stored>, Vec<&'a Stored>);

/// The year's PIT-38 columns.
#[derive(Clone)]
struct Columns { b: Dec, c: Dec, d: Dec, e: Dec, f: Dec }

/// The year the PIT act's crypto regime took effect (art. 44 ust. 1 of the act of 23.10.2018).
pub const FIRST_YEAR: i32 = 2019;

/// D-TAX-PL-DECLARED-CARRY: what a request says of column d. `None`: the field is absent (what is saved stays);
/// `Some(None)`: null or "" (cleared, d is carried from the year before); `Some(Some(d))`: the amount declared. The text
/// of a string or JSON number: digits, at most 13, then at most 2 decimals after a dot (no sign, no exponent; a JSON
/// number of that form round-trips through f64 exactly). `Err` names the refusal.
pub fn carried(raw: Option<&Value>) -> Result<Option<Option<Dec>>, &'static str> {
    let text = match raw {
        None => return Ok(None),
        Some(Value::Null) => return Ok(Some(None)),
        Some(Value::String(s)) if s.is_empty() => return Ok(Some(None)),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(_) => return Err("invalid_carried_costs"),
    };
    let (whole, cents) = text.split_once('.').unwrap_or((&text, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if whole.is_empty() || whole.len() > 13 || !digits(whole) || cents.len() > 2 || !digits(cents) || text.ends_with('.') { return Err("invalid_carried_costs"); }
    Dec::strict(&text).map(|d| Some(Some(d))).map_err(|_| "invalid_carried_costs")
}

/// Saves what a queued request said of column d for (user, year): the caller's transaction, after its refusals.
pub fn declare(tx: &Connection, user: i64, year: i64, value: Option<&Dec>, now: &str) -> rusqlite::Result<()> {
    match value {
        None => tx.execute("DELETE FROM rust_tax_pl_carry WHERE user_id = ?1 AND year = ?2", rusqlite::params![user, year]).map(drop),
        Some(d) => tx.execute("INSERT INTO rust_tax_pl_carry (user_id, year, costs, updated_at) VALUES (?1, ?2, ?3, ?4) \
                               ON CONFLICT (user_id, year) DO UPDATE SET costs = excluded.costs, updated_at = excluded.updated_at",
                              rusqlite::params![user, year, d.round(2).map_err(|_| rusqlite::Error::InvalidQuery)?.to_s_f(), now]).map(drop),
    }
}

/// The amounts declared for the regime's years up to `year`.
pub fn declared(c: &Connection, user: i64, year: i32) -> Result<BTreeMap<i32, Dec>, FiguresError> {
    let mut s = c.prepare("SELECT year, costs FROM rust_tax_pl_carry WHERE user_id = ?1 AND year BETWEEN ?2 AND ?3")?;
    let rows = s.query_map(rusqlite::params![user, FIRST_YEAR, year], |r| Ok((r.get::<_, i32>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = BTreeMap::new();
    for row in rows { let (y, costs) = row?; out.insert(y, Dec::strict(&costs).map_err(|_| FiguresError::Data(format!("a declared PL carry for {y}")))?); }
    Ok(out)
}

impl Report {
    /// The values of every row up to the end of the report's year (local), in row order.
    fn pl_components(&self) -> Vec<Component<'_>> {
        // A group with both a fiat leg and a coin leg is a trade for cash (Kraken's ledger): its fiat legs are the cash.
        let mut groups: HashMap<(i64, &str), Legs> = HashMap::new();
        for r in &self.scoped {
            let Some(g) = r.group.as_deref().filter(|_| r.kind.in_leg() || r.kind.out_leg()) else { continue };
            let legs = groups.entry((r.exchange_id, g)).or_default();
            if fiat(&r.base) { legs.0.push(r) } else { legs.1.push(r) }
        }
        let mut out = vec![];
        for r in &self.scoped {
            let day = self.local(r.at);
            if day.year() > self.year || !(r.kind.in_leg() || r.kind.out_leg()) { continue; }
            let group = r.group.as_deref().and_then(|g| groups.get(&(r.exchange_id, g))).filter(|(f, c)| !f.is_empty() && !c.is_empty());
            let side = |cost: bool| if cost { COST } else { REVENUE };
            // The coin the trade is of, and whether it was for cash.
            let (coin, cash) = match group {
                Some((_, coins)) if fiat(&r.base) => {
                    let coin = coins.iter().find(|c| c.kind.in_leg() == r.kind.out_leg()).unwrap_or(&coins[0]);
                    out.push(Component { row: r, day, asset: coin.base.clone(), amount: Some(coin.amount.clone()), sides: side(r.kind.out_leg()),
                                         money: Money::Cash(r.amount.clone(), r.base.clone()) });
                    (coin.base.clone(), true)
                }
                Some(_) => (r.base.clone(), true),
                None if fiat(&r.base) => continue,
                None => match r.quote.as_deref().filter(|q| fiat(q)).zip(r.quote_amount.as_ref()) {
                    Some((q, paid)) => {
                        out.push(Component { row: r, day, asset: r.base.clone(), amount: Some(r.amount.clone()), sides: side(r.kind.in_leg()),
                                             money: Money::Cash(paid.clone(), q.to_string()) });
                        (r.base.clone(), true)
                    }
                    None => (r.base.clone(), false),
                },
            };
            let Some((fee_currency, fee)) = r.fee_currency.as_deref().filter(|f| !f.trim().is_empty()).zip(r.fee_amount.as_ref()).filter(|(_, a)| a.is_positive()) else { continue };
            if fiat(fee_currency) {
                // A swap's fee in cash is no cost (art. 23 ust. 1 pkt 38d).
                if cash { out.push(Component { row: r, day, asset: coin, amount: None, sides: COST, money: Money::Cash(fee.clone(), fee_currency.to_string()) }); }
            } else {
                out.push(Component { row: r, day, asset: fee_currency.to_string(), amount: Some(fee.clone()), sides: if cash { BOTH } else { REVENUE },
                                     money: Money::CoinFee(fee.clone()) });
            }
        }
        out
    }

    /// The (currency, first day) of every value Poland's report converts at NBP's rates: what the job loads first (none
    /// for another country).
    pub fn nbp_needs(&self) -> Vec<(String, NaiveDate)> {
        if self.nbp.is_none() { return vec![]; }
        let mut first: HashMap<String, NaiveDate> = HashMap::new();
        for c in self.pl_components() {
            let currency = match &c.money { Money::Cash(_, cur) => cur.as_str(), Money::CoinFee(_) => "USD" };
            if currency == "PLN" { continue; }
            let day = first.entry(currency.to_string()).or_insert(c.day);
            if c.day < *day { *day = c.day; }
        }
        let mut out: Vec<(String, NaiveDate)> = first.into_iter().collect();
        out.sort();
        out
    }

    /// The PL file: the year's values, the PIT-38 columns, the year table, the notes.
    pub(super) fn pl_compute(&self, fetcher: &mut dyn Fetcher, today: NaiveDate) -> Result<Outcome, FiguresError> {
        let empty = nbp::Nbp::default();
        let rates = self.nbp.as_ref().unwrap_or(&empty);
        let mut ps = PriceService::new(&self.reference, &self.fx, today, fetcher);
        ps.record(&self.scoped);
        let mut valued = vec![];
        for c in self.pl_components() {
            let (original, currency, mut missing) = match &c.money {
                Money::Cash(amount, currency) => (amount.clone(), currency.clone(), false),
                // Its dollar value: the coin's daily USD price (the app's price table) times the amount.
                Money::CoinFee(amount) => {
                    let before = ps.warnings.len();
                    let price = ps.price_at(&c.asset, "USD", c.row.at, Some(&c.row.venue))?;
                    ((amount * &price)?, "USD".to_string(), ps.warnings.len() > before)
                }
            };
            let rate = if currency == "PLN" { Some((None, Dec::one())) } else { rates.before(&currency, c.day).map(|(d, r)| (Some(d), r.clone())) };
            missing |= rate.is_none();
            let value = match &rate { Some((_, r)) => (&original * r)?.round(2)?, None => Dec::zero() };
            valued.push(Valued { c, original, currency, rate, value, missing });
        }

        // Each year's b and c; costs before the regime apart.
        let mut years: BTreeMap<i32, (Dec, Dec)> = BTreeMap::new();
        let mut before_regime = Dec::zero();
        for v in &valued {
            for side in v.c.sides {
                let y = v.c.day.year();
                if y < FIRST_YEAR { if *side == Side::Cost { before_regime = (&before_regime + &v.value)?; } continue; }
                let (b, c) = years.entry(y).or_insert_with(|| (Dec::zero(), Dec::zero()));
                if *side == Side::Revenue { *b = (&*b + &v.value)?; } else { *c = (&*c + &v.value)?; }
            }
        }
        // The chain of column f into next year's d, from the first year of synced values or of a declared d, whichever is
        // earlier; a declared d replaces the carried f in its year (D-TAX-PL-DECLARED-CARRY).
        let start = years.keys().chain(self.carry.keys()).min().copied().unwrap_or(self.year).min(self.year);
        let mut chain: Vec<(i32, Columns, bool)> = vec![];
        let mut carried = Dec::zero();
        for y in start..=self.year {
            let declared = self.carry.get(&y);
            if let Some(d) = declared { carried = d.clone(); }
            let (b, c) = years.get(&y).cloned().unwrap_or_else(|| (Dec::zero(), Dec::zero()));
            let costs = (&c + &carried)?;
            let net = (&b - &costs)?;
            let (e, f) = if net.is_negative() { (Dec::zero(), net.neg()) } else { (net, Dec::zero()) };
            chain.push((y, Columns { b, c, d: carried, e, f: f.clone() }, declared.is_some()));
            carried = f;
        }
        let this = chain.last().map(|(_, c, _)| c.clone()).ok_or_else(|| FiguresError::Data("no report year".into()))?;
        let base = this.e.round(0)?;
        let tax = (&base * &Dec::strict("0.19")?)?.round(0)?;
        let warnings = { let mut seen = HashSet::new(); ps.warnings.iter().filter(|w| seen.insert(w.as_str())).cloned().collect::<Vec<_>>() };
        self.pl_csv(&valued, &chain, &this, &base, &tax, &before_regime, &warnings).map(Outcome::Csv)
    }

    #[allow(clippy::too_many_arguments)]
    fn pl_csv(&self, valued: &[Valued], chain: &[(i32, Columns, bool)], this: &Columns, base: &Dec, tax: &Dec, before_regime: &Dec,
              warnings: &[String]) -> Result<String, FiguresError> {
        let l = self.j.locale;
        let mut out = String::new();
        let keys = ["date", "asset", "amount", "kind", "original_amount", "original_currency", "nbp_rate", "nbp_table_date", "value_pln", "exchange",
                    "tx_id", "data_incomplete"];
        line(&mut out, &keys.iter().map(|k| self.t(&format!("tax_report.headers.{k}"))).collect::<Vec<_>>());
        for b in &self.banners { line(&mut out, &[s(b.clone())]); }
        // Every value the columns read (each year of the chain, and before it) counts when it is missing.
        let missing: usize = valued.iter().filter(|v| v.missing).map(|v| v.c.sides.len()).sum();
        if missing > 0 {
            let banner = text(l, "tax_report.incomplete_banner", &[("missing", Arg::Text(&missing.to_string()))]);
            line(&mut out, &[s(format!("{}: {banner}", text(l, "tax_report.incomplete_banner_prefix", &[])))]);
        }
        let this_year: Vec<&Valued> = valued.iter().filter(|v| v.c.day.year() == self.year).collect();
        if this_year.is_empty() { line(&mut out, &[self.t("tax_report.no_taxable_transactions")]); }
        for v in &this_year {
            for side in v.c.sides {
                let kind = if *side == Side::Revenue { "revenue" } else { "cost" };
                let (rate, table) = match &v.rate { Some((d, r)) => (Cell::Raw(r.to_s_f()), d.map_or(Cell::Nil, |d| s(d.to_string()))), None => (Cell::Nil, Cell::Nil) };
                line(&mut out, &[s(v.c.day.to_string()), s(v.c.asset.clone()), v.c.amount.as_ref().map_or(Cell::Nil, n), self.t(&format!("tax_report.values.{kind}")),
                                 n(&v.original), s(v.currency.clone()), rate, table, cents(&v.value)?, s(v.c.row.venue.clone()),
                                 v.c.row.tx_id.clone().map_or(Cell::Nil, s), Cell::Raw(v.missing.to_string())]);
            }
        }

        line(&mut out, &[]);
        line(&mut out, &[self.t("tax_report.pit38.header")]);
        // D-TAX-PL-DECLARED-CARRY: with no declared year in the chain, the file is as it was.
        let declared = chain.iter().any(|(_, _, d)| *d);
        let this_declared = chain.last().is_some_and(|(_, _, d)| *d);
        for (key, value) in [("b", &this.b), ("c", &this.c), ("d", &this.d), ("e", &this.e), ("f", &this.f)] {
            line(&mut out, &[self.t(&format!("tax_report.pit38.{key}")), cents(value)?]);
            if key == "d" && this_declared { line(&mut out, &[self.t("tax_report.pit38.d_declared")]); }
        }
        line(&mut out, &[self.t("tax_report.pit38.base"), Cell::Raw(zloty(base))]);
        line(&mut out, &[self.t("tax_report.pit38.tax"), Cell::Raw(zloty(tax))]);
        line(&mut out, &[]);
        line(&mut out, &[self.t(if declared { "tax_report.pit38.years_header_declared" } else { "tax_report.pit38.years_header" })]);
        line(&mut out, &[Cell::Nil, s("b"), s("c"), s("d"), s("e"), s("f")]);
        for (y, c, d) in chain {
            let mut cells = vec![Cell::Raw(y.to_string()), cents(&c.b)?, cents(&c.c)?, cents(&c.d)?, cents(&c.e)?, cents(&c.f)?];
            if *d { cells.push(self.t("tax_report.pit38.declared")); }
            line(&mut out, &cells);
        }
        line(&mut out, &[self.t(if declared { "tax_report.pit38.carried_hint_declared" } else { "tax_report.pit38.carried_hint" })]);
        if before_regime.is_positive() {
            let first_declared = chain.iter().any(|(y, _, d)| *y == FIRST_YEAR && *d);
            line(&mut out, &[]);
            line(&mut out, &[self.t("tax_report.pit38.before_2019"), cents(before_regime)?]);
            line(&mut out, &[self.t(if first_declared { "tax_report.pit38.before_2019_hint_declared" } else { "tax_report.pit38.before_2019_hint" })]);
        }

        if !warnings.is_empty() {
            line(&mut out, &[]);
            line(&mut out, &[s(format!("WARNING: {}:", text(l, "tax_report.warnings.missing_prices", &[])))]);
            for w in warnings { line(&mut out, &[s(w.clone())]); }
            line(&mut out, &[self.t("tax_report.warnings.upgrade_hint")]);
        }
        let unrated = valued.iter().filter(|v| v.rate.is_none()).map(|v| format!("{} {}", v.currency, v.c.day));
        self.block(&mut out, "WARNING", "missing_nbp_rates", "missing_nbp_rates_hint", unrated)?;
        let listed = |r: &Stored| format!("{} {} {}", r.base, r.amount.to_s_f(), date(r.at));
        let until = |r: &Stored| self.local(r.at).year() <= self.year;
        let linked: HashSet<i64> = self.scoped.iter().filter_map(|r| r.linked_to).collect();
        self.block(&mut out, "WARNING", "unlinked_deposits", "deposit_costs_missing_hint",
                   self.scoped.iter().filter(|r| r.kind == Kind::Deposit && !linked.contains(&r.id) && !fiat(&r.base) && until(r)).map(listed))?;
        self.block(&mut out, "WARNING", "unlinked_withdrawals", "unlinked_withdrawals_hint_pl",
                   self.scoped.iter().filter(|r| r.kind == Kind::Withdrawal && r.linked_to.is_none() && !fiat(&r.base) && until(r)).map(listed))?;
        let in_year = |r: &Stored| self.local(r.at).year() == self.year && !fiat(&r.base) && r.amount.is_positive();
        self.block(&mut out, "NOTE", "rewards_not_revenue", "rewards_not_revenue_hint",
                   self.scoped.iter().filter(|r| r.kind.in_kind() && in_year(r)).map(listed))?;
        // A coin fee of a row of its own names no trade: neither its revenue nor whether it is a cost can be told.
        self.block(&mut out, "NOTE", "coin_fees", "coin_fees_hint_pl", self.scoped.iter().filter(|r| r.kind == Kind::Fee && in_year(r)).map(listed))?;
        Ok(out)
    }
}

/// Whole złoty, as the form writes them.
fn zloty(d: &Dec) -> String { let s = d.to_s_f(); s.strip_suffix(".0").map_or_else(|| s.clone(), str::to_string) }

