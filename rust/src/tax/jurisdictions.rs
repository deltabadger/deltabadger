//! Tax::Jurisdictions: REGISTRY as data (every flag of every entry, in Rails' order), `holding_period`, `held_long?`,
//! and the report currency of a year (Tax::Report#currency). BotApi::Tax::ListJurisdictions reads this table.
use chrono::{Days, Months, NaiveDate};

/// A Duration as ActiveSupport keeps it: its calendar parts (`1.year`, `365.days`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Period { pub years: u32, pub days: u64 }

/// `wealth_tax` of one year.
#[derive(Clone, Copy, Debug)]
pub struct WealthYear { pub year: i32, pub allowance: i64, pub deemed_return: &'static str, pub rate: &'static str }

/// One REGISTRY entry. A flag Rails leaves out is None / false here; decimals are kept as Rails writes them.
#[derive(Clone, Copy, Debug)]
pub struct Jurisdiction {
    pub code: &'static str,
    pub name: &'static str,
    pub method: &'static str,
    pub currency: &'static str,
    pub locale: &'static str,
    /// None where Rails has no key (the engines' default: taxable).
    pub crypto_to_crypto_taxable: Option<bool>,
    pub stablecoin_ambiguous: bool,
    pub holding_exemption: Option<Period>,
    pub holding_exemption_inclusive: bool,
    pub swap_resets_holding_period: bool,
    pub old_stock_cutoff: Option<(i32, u32, u32)>,
    pub short_long_term: bool,
    /// (before, after, cutoff).
    pub tax_rate: Option<(&'static str, &'static str, (i32, u32, u32))>,
    /// (max_year, amount).
    pub exemption_threshold: Option<(i32, i64)>,
    pub expense_deduction: Option<&'static str>,
    pub currency_by_year: &'static [(i32, &'static str)],
    pub flat_tax_rate: Option<&'static str>,
    pub wealth_tax: &'static [WealthYear],
    /// `snapshot_date: :end_of_year`.
    pub snapshot_end_of_year: bool,
    pub summary_only_total: bool,
    pub income_taxed_separately: bool,
    pub annual_threshold: Option<i64>,
    pub wash_sale: Option<(u32, &'static str)>,
    pub loss_deduction_rate: Option<&'static str>,
    pub annual_exemption: Option<i64>,
    pub split_payment: bool,
    pub danish_wash_sale: bool,
    pub per_asset_summary: bool,
    pub loss_deduction_rate_on_losses: Option<&'static str>,
    pub czech_exemptions: Option<Czech>,
}

/// `czech_exemptions`.
#[derive(Clone, Copy, Debug)]
pub struct Czech { pub time_test: Period, pub exemptions_from: (i32, u32, u32), pub value_test: i64, pub annual_cap: i64 }

const YEAR: Period = Period { years: 1, days: 0 };

const BASE: Jurisdiction = Jurisdiction {
    code: "", name: "", method: "fifo", currency: "EUR", locale: "en", crypto_to_crypto_taxable: None, stablecoin_ambiguous: false,
    holding_exemption: None, holding_exemption_inclusive: false, swap_resets_holding_period: false, old_stock_cutoff: None,
    short_long_term: false, tax_rate: None, exemption_threshold: None, expense_deduction: None, currency_by_year: &[], flat_tax_rate: None,
    wealth_tax: &[], snapshot_end_of_year: false, summary_only_total: false, income_taxed_separately: false, annual_threshold: None,
    wash_sale: None, loss_deduction_rate: None, annual_exemption: None, split_payment: false, danish_wash_sale: false,
    per_asset_summary: false, loss_deduction_rate_on_losses: None, czech_exemptions: None,
};

/// Tax::Jurisdictions::REGISTRY, in its order.
pub const REGISTRY: [Jurisdiction; 18] = [
    Jurisdiction { code: "DE", name: "Germany", locale: "de", holding_exemption: Some(YEAR), ..BASE },
    Jurisdiction { code: "AT", name: "Austria", locale: "de", crypto_to_crypto_taxable: Some(false), stablecoin_ambiguous: true,
                   old_stock_cutoff: Some((2021, 3, 1)), ..BASE },
    Jurisdiction { code: "FR", name: "France", method: "pvct", locale: "fr", crypto_to_crypto_taxable: Some(false), annual_threshold: Some(305), ..BASE },
    Jurisdiction { code: "IT", name: "Italy", method: "lifo", locale: "it", exemption_threshold: Some((2024, 2000)),
                   tax_rate: Some(("26%", "33%", (2026, 1, 1))), ..BASE },
    Jurisdiction { code: "ES", name: "Spain", locale: "es", ..BASE },
    Jurisdiction { code: "BG", name: "Bulgaria", locale: "bg", expense_deduction: Some("0.1"), currency_by_year: &[(2025, "BGN")], ..BASE },
    Jurisdiction { code: "GR", name: "Greece", locale: "el", flat_tax_rate: Some("0.15"), ..BASE },
    Jurisdiction { code: "NL", name: "Netherlands", method: "wealth_snapshot", locale: "nl",
                   wealth_tax: &[WealthYear { year: 2025, allowance: 57_684, deemed_return: "0.0588", rate: "0.36" },
                                 WealthYear { year: 2026, allowance: 59_357, deemed_return: "0.0778", rate: "0.36" }], ..BASE },
    // Art. 10 CIRS: 365 days or more, inclusive.
    Jurisdiction { code: "PT", name: "Portugal", locale: "pt", crypto_to_crypto_taxable: Some(false), holding_exemption: Some(Period { years: 0, days: 365 }),
                   holding_exemption_inclusive: true, swap_resets_holding_period: true, ..BASE },
    Jurisdiction { code: "CH", name: "Switzerland", method: "wealth_snapshot", currency: "CHF", locale: "de", snapshot_end_of_year: true,
                   summary_only_total: true, income_taxed_separately: true, ..BASE },
    Jurisdiction { code: "PL", name: "Poland", currency: "PLN", locale: "pl", crypto_to_crypto_taxable: Some(false), ..BASE },
    Jurisdiction { code: "GB", name: "United Kingdom", method: "share_pooling", currency: "GBP", locale: "en", wash_sale: Some((30, "UK")), ..BASE },
    Jurisdiction { code: "US", name: "United States", currency: "USD", locale: "en", short_long_term: true, wash_sale: Some((30, "US")), ..BASE },
    Jurisdiction { code: "SE", name: "Sweden", method: "weighted_average", currency: "SEK", locale: "sv", loss_deduction_rate: Some("0.7"), ..BASE },
    Jurisdiction { code: "IE", name: "Ireland", method: "fifo_4week", locale: "en", annual_exemption: Some(1270), split_payment: true,
                   wash_sale: Some((28, "IE")), ..BASE },
    Jurisdiction { code: "DK", name: "Denmark", currency: "DKK", locale: "da", danish_wash_sale: true, per_asset_summary: true,
                   loss_deduction_rate_on_losses: Some("0.26"), ..BASE },
    Jurisdiction { code: "CZ", name: "Czech Republic", currency: "CZK", locale: "cs",
                   czech_exemptions: Some(Czech { time_test: Period { years: 3, days: 0 }, exemptions_from: (2025, 2, 15), value_test: 100_000,
                                                  annual_cap: 40_000_000 }), ..BASE },
    // No holding period: the 7% rate was repealed before it took effect (Act 530/2023).
    Jurisdiction { code: "SK", name: "Slovakia", locale: "sk", crypto_to_crypto_taxable: Some(false), stablecoin_ambiguous: true, ..BASE },
];

/// `Jurisdictions.for(code)`.
pub fn find(code: &str) -> Option<&'static Jurisdiction> { REGISTRY.iter().find(|j| j.code == code) }

/// D-TAX-NOT-PORTED-GATE: the jurisdictions whose crypto report this build computes. Until a jurisdiction's slice lands,
/// a create for it (web, REST, MCP) is refused up front as not ported, and nothing is ever queued for a report this
/// build cannot compute; the broker scope likewise until A12-7. Each later slice adds its countries; the gate goes with A12-7.
/// AT and PL are not: Austria's moving average per wallet for holdings from 2023 and its staking rules, and Poland's
/// costs deducted in the year paid (carried forward), are not this engine's per-sale FIFO.
pub const PORTED: [&str; 5] = ["US", "DE", "ES", "PT", "SK"];

pub fn ported(country: &str) -> bool { PORTED.contains(&country) }

pub(crate) fn ymd((y, m, d): (i32, u32, u32)) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, d).unwrap_or(NaiveDate::MIN) }

impl Jurisdiction {
    /// Tax::Report#currency: `currency_by_year[year] || currency`.
    pub fn currency(&self, year: i32) -> &'static str {
        self.currency_by_year.iter().find(|(y, _)| *y == year).map_or(self.currency, |(_, c)| c)
    }

    /// `Jurisdictions.holding_period`: (period, inclusive), or None where the law has none.
    pub fn holding_period(&self) -> Option<(Period, bool)> {
        if let Some(p) = self.holding_exemption { return Some((p, self.holding_exemption_inclusive)); }
        if let Some(cz) = self.czech_exemptions { return Some((cz.time_test, false)); }
        (self.short_long_term || self.old_stock_cutoff.is_some()).then_some((YEAR, false))
    }

    /// Whether swapping a coin for a coin is a disposal (`options.fetch(:crypto_to_crypto_taxable, true)`).
    pub fn swaps_taxable(&self) -> bool { self.crypto_to_crypto_taxable.unwrap_or(true) }
}

/// `Jurisdictions.held_long?`: both dates in the taxpayer's zone; the period ends on `Date#advance(period.parts)` (29
/// February's year anniversary is 28 February); held long is any later date, or that date too when inclusive.
pub fn held_long(acquired: NaiveDate, disposed: NaiveDate, period: Period, inclusive: bool) -> bool {
    let ends = acquired.checked_add_months(Months::new(period.years * 12)).and_then(|d| d.checked_add_days(Days::new(period.days)));
    match ends { Some(ends) => if inclusive { disposed >= ends } else { disposed > ends }, None => false }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn d(s: &str) -> NaiveDate { NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap() }

    #[test]
    fn the_registry_is_rails_order_and_the_ported_set_is_fifo_without_a_summary() {
        assert_eq!(REGISTRY.map(|j| j.code), crate::web::tracker::modal::COUNTRIES);
        for c in PORTED { let j = find(c).unwrap(); assert_eq!(j.method, "fifo", "{c}"); assert!(j.expense_deduction.is_none() && !j.per_asset_summary && j.czech_exemptions.is_none() && j.flat_tax_rate.is_none()); }
        assert_eq!(find("BG").unwrap().currency(2025), "BGN");
        assert_eq!(find("BG").unwrap().currency(2026), "EUR");
    }

    #[test]
    fn holding_periods_end_on_the_calendar_anniversary() {
        let year = find("DE").unwrap().holding_period().unwrap();
        assert_eq!(year, (YEAR, false));
        assert!(!held_long(d("2024-02-29"), d("2025-02-28"), year.0, false));
        assert!(held_long(d("2024-02-29"), d("2025-03-01"), year.0, false));
        let pt = find("PT").unwrap().holding_period().unwrap();
        assert!(pt.1 && held_long(d("2024-02-29"), d("2025-02-28"), pt.0, true));
        assert!(!held_long(d("2025-03-11"), d("2026-03-10"), pt.0, true));
        assert_eq!(find("AT").unwrap().holding_period(), Some((YEAR, false)));
        assert_eq!(find("CZ").unwrap().holding_period().unwrap().0, Period { years: 3, days: 0 });
        assert!(find("SK").unwrap().holding_period().is_none() && find("ES").unwrap().holding_period().is_none());
    }
}
