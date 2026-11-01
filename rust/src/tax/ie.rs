//! Ireland (TCA 1997 ss. 28, 31, 546, 581, 601, 959AM): the CGT summary and its payment periods (A12-4). The engine's part
//! is `fifo::Options::ireland` (s. 581's matching and loss restriction).

/// D-TAX-IE-YEARS: the first full year at 33% (FA 2013 s. 43).
pub const FIRST_YEAR: i32 = 2013;
/// D-TAX-IE-YEARS: the last year whose law was reviewed.
pub const LAST_YEAR: i32 = 2026;
