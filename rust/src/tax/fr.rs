//! France (CGI art. 150 VH bis; BOI-RPPM-PVBMC-30; form 2086): the PVCT report (A12-6). The engine's part is
//! `fifo::Options::pvct_from` (the units held before each cession and the running acquisition price).

/// D-TAX-FR-YEARS: the first and last years whose law was reviewed (150 VH bis applies to cessions from 1 January 2019).
pub const FIRST_YEAR: i32 = 2019;
pub const LAST_YEAR: i32 = 2026;
