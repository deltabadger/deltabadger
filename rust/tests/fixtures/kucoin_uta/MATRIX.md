# KuCoin UTA contract and acceptance matrix (issue #841)

UTA-0 supplies the fixtures and the pure contract checks in `rust/tests/kucoin_uta.rs`. A detected UTA account
(`/api/ua/v2/account/mode` = UNIFIED) stays refused until U4. The rows below are acceptance specifications for U1–U4;
each slice first writes the named failing test on these assets with `ScriptedTransport`, then the behavior. Every money
assertion inspects persisted effects, including after reopening the store.

`manifest.json` cites the official doc pages (fetched 2026-10-31, each as its `.md` form) and the commit-pinned SDK,
lists the endpoint contracts, and pins the generator and every generated file. The docs' examples are mostly FUTURES:
each page's success example is kept verbatim as text under `source_examples`, and every case is a labelled synthetic
SPOT variant. All credentials are throwaway text.

Regenerate with `python3 script/rust/record_kucoin_uta.py --source-dir <cache>`; add `--check` to compare without
writing. The cache is the doc pages' `.md` files plus the SDK's `generate/uta/base_api.py`.

## Executable now: UTA-0

| Contract test (`kucoin_uta_` prefix) | Assertions |
|---|---|
| `provenance_and_generated_assets_are_pinned` | SHA-256 of generator and every generated file; source URLs, fetch date, SDK commit; unique case ids; case counts. |
| `endpoint_contract_covers_every_planned_route` | Mode, place, cancel, detail, execution, unified balance, api-key; tradeType required; retention and UNKNOWNs recorded. |
| `status_cases_agree_with_filled_size` | Status 2–6 only; 3/6 need filledSize > 0, 2/5 need 0; every other shape or identity mismatch is unreadable. |
| `totals_come_from_unique_validated_executions` | Quote = Σ value over tradeId-unique fills of this order, symbol and side; Σ size = filledSize exactly, else waiting/unreadable. |
| `fill_walk_is_account_wide_with_exact_cursor` | 203 unique of 204 read, cursors above 2^53 kept exact, empty-page end, BUY/SELL only. |
| `balance_cases_never_spend_unpnl_or_show_debt` | Spendable = min(available, balance − hold); liability or negative balance refuses. |
| `independent_hmac_vectors_cover_get_query_and_post_body` | Ordered unescaped prehash, compact JSON body, Base64 HMAC and passphrase, form-encoded URL. |

## Activation and assertions for implementation slices (plan UTA.md, revisions r1–r5 override its body)

| Row | Slice | Failing test first | Must hold |
|---|---|---|---|
| G01 | U1 | `kucoin_uta_sync::g01_*` | `/api/ua/` requests sign as `signing.json` through the real transport. |
| B01 | U1 | `b01_*` (balance sync) | `balances.json`: held = balance; refused cases keep the previous rows. |
| L01 | U1 | `l01_*` | `fills.json` walk publishes 203 rows once; every failing case publishes nothing from its page and records no `uta_read`. |
| L02 | U1 | `l02_*` | First UTA detection migrates once (`hf_enabled` marker); a seeded classic cutoff no longer hides CSV fills. |
| L03 | U1 | `l03_*` | Only the current mode certifies, from its own episode start; HF → UTA → HF leaves `[t0, t1)` on the gap line. |
| L04 | U1 | `l04_*` | Backfill reads unread 7-day windows newest first within 365 days, ignores exports, retries a failed window after 24 h. |
| L05 | U1 | `l05_*` | Classic fills refusing on UTA: deposits and withdrawals still import; the watermark holds. |
| L06 | U1 | `l06_*` | An unknown-type export row at a second holding an API fill of that currency is withheld and flagged. |
| O01 | U2 | `kucoin_uta::o01_*` | `place_*` bodies; accepted → one row + record `family: "Uta"`; restart sends no second POST. |
| O02 | U2 | `o02_*` | Every `errors.json` case → its intent/row effect. |
| O03 | U2 | `o03_*` | `status_*` and `totals_*` through the adapter; engine balance spends per `balances.json`. |
| O04 | U2 | `o04_*` | `recovery_*`: a miss keeps the intent. `cancel_*`: never terminal. |
| R01 | U3 | `r01_*` | Unreadable UTA row holds spending; terminal read settles once; `reconcile-order` needs equal family. |
| R02 | U3 | `r02_*` | Switch marks: classic row → UTA → classic stays protected; warm-cache cancel keeps both obligations. |
| F01 | U4 | `g01_*` (real factory) | DCA places, polls and settles on UTA; classic and HF twins unchanged; `apikey.json` verdicts; the UTA refusal copy is gone. |
