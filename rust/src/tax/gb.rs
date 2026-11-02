//! The United Kingdom (TCGA 1992 ss. 1, 1H, 1K, 104, 105, 106A, 288(1ZA); HMRC Cryptoassets Manual): the SA108 summary
//! and its notes (A12-5). The engine's part is `fifo::Options::average_all` (the s. 104 pool) with the identification,
//! `lost_kept`, `own_fee_disposed`, `swap_fee_split` and `defer_group_fees`.

/// D-TAX-GB-YEARS: the tax years served, by the year they start in (2020 = 6 April 2020 to 5 April 2021).
pub const FIRST_YEAR: i32 = 2020;
pub const LAST_YEAR: i32 = 2026;
