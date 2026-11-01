//! Italy (TUIR art. 67 c. 1 lett. c-sexies, art. 68 c. 9-bis; L. 197/2022, L. 207/2024, L. 199/2025; Circolare 30/E 2023):
//! the quadro RT summary and the euro e-money token rules (A12-4). The engine's part is `fifo::Options::{lifo,
//! fees_ignored_from, swaps_taxable_before}`.

/// D-TAX-IT-YEARS: the crypto-asset regime starts on 1 January 2023 (L. 197/2022 c. 126).
pub const FIRST_YEAR: i32 = 2023;
/// D-TAX-IT-YEARS: the TUIR is repealed from 2027 (D.Lgs. 117/2026), not yet reviewed.
pub const LAST_YEAR: i32 = 2026;
