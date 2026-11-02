//! Sweden (IL 44, 48 kap. 7 §, 52 kap. 3, 5 §§, 65 kap. 7 §, 67 kap. 10 §; Skatteverket "Kryptotillgångar"): the K4
//! avsnitt D summary and its notes (A12-5). The engine's part is `fifo::Options::average_all` (genomsnittsmetoden, one
//! average per coin per person) with `lost_kept`, `own_fee_disposed` and `defer_group_fees`.

/// D-TAX-SE-YEARS: the first and last years whose law was reviewed.
pub const FIRST_YEAR: i32 = 2020;
pub const LAST_YEAR: i32 = 2026;
