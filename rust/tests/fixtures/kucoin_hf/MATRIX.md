# KuCoin HF contract and acceptance matrix

HF-0 implements only the fixture/contract checks in `rust/tests/kucoin_hf.rs`.
The behavior rows below are acceptance specifications for HF-1–5, not passing
adapter, database, concurrency or parity tests. They carry forward the reviewed
AUDIT #10 plan, including the history-import rulings. No HF runtime path is enabled.

`manifest.json` cites the fetched official documentation and SDK revision/file
hashes, lists endpoint contracts, and pins the generator and generated files.
Each case identifies its sources and synthetic changes. The original examples
remain separate: request/response IDs and timestamps in the docs are not always
consistent. `response_edits` lists JSON-pointer edits relative to those originals;
HTTP statuses, fault messages, clocks, coherent requests and generated page walks
are synthetic unless explicitly attributed. All credentials are throwaway text.

Regenerate with `python3 script/rust/record_kucoin_hf.py --source-dir <cache>`;
add `--check` to compare without writes or network access. Retain the fetched
cache to reproduce the dated documentation snapshot. SDK URLs are commit-pinned.
Run `script/rust/repin --check` after generator changes; no Rails recorder or source
pin is changed by HF-0. Python's standard-library HMAC generates the expectations;
the Rust tests recompute them with the existing Rust crypto dependencies. They do
not call the classic signer and cannot establish HF transport correctness yet.

## Executable now: HF-0

| Contract test (`kucoin_hf_` prefix) | Assets / assertions |
|---|---|
| `provenance_and_generated_assets_are_pinned` | SHA-256 of generator and every generated data file; official source URLs, fetch date and SDK commit; unique synthetic case IDs and summed case counts. |
| `endpoint_contract_covers_every_planned_route` | Mode, opened, trade_hf balances, API-key permission, placement, both status routes, fills and cancel; symbol required on status/fills/cancel. |
| `source_examples_remain_separate_from_coherent_scenarios` | Preserve original false opened flag, classic balance sample, mismatched status IDs/clientOids and out-of-range fill time; exact integer tradeId. |
| `detection_contract_never_guesses_or_synthesizes_zero` | CLASSIC/false, CLASSIC/true, UNIFIED; unknown/malformed/failing detection creates no intent/POST, preserves balances; holds/locked totals; General alone is insufficient. |
| `wire_cases_preserve_intent_uuid_and_sizing_inputs` | Buy/sell × market size/funds/limit; exact persisted UUID and already-sized decimal strings; acceptance obligations. |
| `status_cancel_and_identity_contract_is_conservative` | Active before/in book, filled, cancelled partial, invalid flags/amounts, rounding; zero fill at 59 s, exactly 60 s, and 60 s + 1 ms; cancel acknowledgment does not prove terminal state; mismatch/miss retains intent. |
| `placement_classification_pins_database_obligations` | Concrete rejection codes/messages, ambiguous errors including 500000/230005 at multiple HTTP statuses, duplicate ID, malformed success, lost reply; expected intent/row/reserve obligations are data, not observed writes. |
| `paging_fixture_requires_returned_integer_cursor_and_empty_terminator` | 100 + 3 − 1 duplicate = 102 fills; short page continues to empty; cursor and tradeId differ, both exceed 2^53; faults preserve watermark/CSV and publish zero API rows. |
| `fill_amounts_fees_and_symbol_identity_are_exact` | Separate buy/sell orders with consistent sides; exact price × size = funds, aggregate base/quote/fees without the repeated boundary fill; same tradeId on different symbols gives two ledger identities. |
| `independent_hmac_vectors_include_delete_query_and_exact_body` | GET mode/opened/balances/both status routes/fills, POST market/limit and DELETE with symbol; ordered unescaped prehash, exact compact JSON body, Base64 HMAC/passphrase and separately encoded URL. Extra remark is an authentication encoding probe, not an accepted endpoint parameter claim. |

## Activation and assertions for implementation slices

All rows below are **deferred behavior assertions**. In the named slice, first
write the indicated failing test using these assets and `ScriptedTransport`, then
implement the behavior and include the test in the normal suite. Do not replace
these with ignored always-passing tests or a fake HF implementation in a test.
HF-0's pure checks remain enabled throughout; they are not a substitute for these
integration assertions. HF-5 must have no deferred rows left.

`grid-kucoin-hf` is the later Rust-only replay runner, with the documented contract
as its oracle. It must compare actual sends, placement intents, transaction rows,
balances, ledger rows, cursors and diagnostics against expectations. It must never
call an invented Rails HF path. Preserve exact decimal strings and raw IDs. Every
money-path test inspects persisted effects, including after reopening the store;
an adapter error alone does not satisfy the row.

| ID / slice / target | Fixture or setup and trigger | Required observed assertions |
|---|---|---|
| M01 / HF-1 / `kucoin_hf` | `account`: `detect_classic`, `detect_hf`, `detect_uta`; `mode_*`, `opened_*` faults | Literal CLASSIC + false selects ClassicLf; CLASSIC + true selects ClassicHf; UNIFIED refuses without opened lookup. Unknown mode, timeout, 401/403/429, bad envelope and absent/nonboolean opened produce zero placement POSTs/intents; reason is visible/redacted. No guessed mode or zero balance. |
| M02 / HF-1 / `kucoin_hf` | Two bots share key/host; another key and host; concurrent resolver calls | One single-flight detection for the shared version; no cross-key/host reuse. Success cached at most five minutes; restart/expiry forces detection. Error invalidates cache; failed refresh never returns stale success. |
| M03 / HF-1 / `kucoin_hf`, `placement`, `tracker_sync` | Replace key, secret or passphrase during detection, balance read, intent creation, response handling or sync checkpoint | CredentialVersion/Produced fences reject every old-version result/write. Rotation invalidates cache; no relabelling old evidence with the new version. Fresh key validation invalidates its entry. |
| M04 / HF-1 / `placement` | Preparation for DCA, signal, rebalance, liquidation, redeploy; expires between preparation and send; mode changes during refresh | Preparation precedes sizing/balances and intent commit. Persist immutable prepared family with UUID. Expired preparation must refresh to the same family; otherwise zero sends. Reuse default-no-op venue preparation, not IBKR PlacementHold. |
| M05 / HF-1 / `kucoin_hf`, `sync_parity` | Successful classic detection around all existing classic scenarios | Assert the two detection sends separately, then exact subsequent requests and DB effects. Preserve all 172 grid-kucoin cases, signing/sizing/status vectors, balances/ledger sync and key verdicts; never widen divergence validators to hide extra sends. |
| O01 / HF-2 / `kucoin_hf`, `placement` | Six `place_{buy,sell}_{market_size,market_funds,limit}` cases | Reuse classic sizing/rounding; exact persisted UUID in wire bytes. Accepted response inserts one row, transfers context and clears intent atomically. Keep raw orderId; never repeat POST on replay/restart. |
| O02 / HF-2 / `kucoin_hf`, `placement` | Every `errors` case and accepted placement | Auth 400003–400007, parameter 400100 excluding duplicate ID, 200004 and 429000/HTTP429 prove refusal. 400002 safely reschedules with no row. Proven pre-send failure is NotSent. MaybeSent, 5xx, malformed/missing code or ID, unknown code, duplicate ID and 500000/230005 keep intent and block replacement. Inspect actual DB effects. Classic classification unchanged. |
| O03 / HF-2 / `kucoin_hf` | All ten `signing` vectors through actual HTTP transport | Assert sent method, path, ordered query and body bytes, signature, timestamp, key version and encrypted passphrase. GET/DELETE sign unescaped parameters while URL encoding is checked separately. Omitting DELETE symbol/query must fail; classic delete_query=false remains unchanged. |
| O04 / HF-2 / `kucoin_hf`, `polling` | `status_active_*`, `status_filled`, `status_cancelled_partial` | HF reads typed active/inOrderBook, never isActive. Active before book remains Open. Inactive filled closes; cancelled partial retains gross base 0.000004 and quote 0.2708704, with fees separately accounted. |
| O05 / HF-2 / `kucoin_hf`, `polling` | Missing/string active, inactive in book, missing/contradictory execution; add missing/string inOrderBook and cancellation flag | Unresolved, no close/reserve credit. Strict required decimal fields and mutually consistent state. Malformed fields never default to terminal/zero. |
| O06 / HF-2 / `kucoin_hf`, `polling` | `status_zero_59s`, `status_zero_60s`, `status_zero_after_60s`, `status_zero_limit`, `status_price_round_half_up` | Explicit terminal market zero settles only strictly after one minute; no inferred zero from missing fields, cancel acknowledgment or missing timestamp. Limit zero stays unknown. dealFunds/dealSize rounds half up to 67717.7 at one decimal; fee conventions unchanged. Preserve D-KUCOIN-ZERO-FILL-FUNDS. |
| O07 / HF-2 / `kucoin_hf`, `reversal` | `cancel_*`; acknowledgment before final status; fill races cancel | Validate returned order ID and symbol query. Refusal rejects only cancel; lost reply/5xx/unreadable reply leaves cancel uncertain. All require final status before credit; partial or complete racing fills retain exact execution. |
| O08 / HF-2 / `placement`, `polling`, `reversal` | Immutable OrderRef raw ID/symbol/family/provenance; bot pair edited, missing symbol, mode changes | Every poll/single-order/fills/cancel caller uses stored order context, not current bot pair or current mode. Missing symbol blocks unsafe route; stored IDs remain raw. Metadata transfers atomically on acceptance/recovery/operator placed; merge/split and restart retain it. |
| L01 / HF-3 / `kucoin_hf_sync` | `pages_boundary_and_large_ids`, `symbol_scoped_trade_identity` | Exact returned lastId (integer >2^53) drives next page, not tradeId. Continue after three-item page until empty. Deduplicate boundary by symbol-tradeId; same tradeId on another symbol remains separate. Assert buy/sell/partial gross quantities, quote funds, fee currency/amount, raw orderId and links. |
| L02 / HF-3 / `kucoin_hf_sync` | Every incomplete `fills` case; cap exhaustion, empty/nonarray data and conflicting duplicate financial fields | Cursor repeat/missing, malformed/failed page, exhausted throttle, caps, unavailable symbol or inconsistent duplicate leave a visible gap and unchanged complete watermark. Keep previous visible rows; captured pages remain staged. Never success on a clamped eight-day window. |
| L03 / HF-3 / `kucoin_hf_sync`, `tracker_sync` | Retention expires before page two or on resume; older gap plus current fills | Check rolling seven-day retention before every page/resume. Use frozen run end, fixed ≤7-day bounds, half-open whole-second units, inclusive API endAt adjustment and boundary dedup. Stage recent data with its own cursor; no complete finalizer or advancing the global watermark across the older gap. |
| L04 / HF-3 / `tracker_sync` | Traded and sold-to-zero pair absent from bots/balances; inactive/delisted historical pair; mode transition | Persist union of current catalog and durable symbols from transactions, ledger/raw data and imports. Preserve unfinished classic walks, classic-era fills and source-separated coverage. Unknown/unavailable historical symbols remain gaps. |
| L05 / HF-3 / `kucoin_hf_sync`, `kucoin_import` | CSV covers two API pages including an aggregate trade and fee; page-one checkpoint then page-two failure; reopen DB | During staging, visible ledger/holdings/cost basis contain CSV only, including fees; staged API contributes zero even without CSV. Cursor/payload checkpoint atomic under credential fence. Restart never promotes partial coverage or duplicates rows. |
| L06 / HF-3 / `kucoin_hf_sync`, `kucoin_import` | Complete L05 interval; crash immediately before/after promotion; repeat completion; import CSV during staging/after promotion; rotate before commit | One fenced write transaction publishes deduplicated API rows, removes only fully covered importer-owned derivations and fee/adjustment legs, repairs/nullifies links, updates coverage/selection and consumes staging receipt. Exact holdings/cost basis remain equal before/after. Retry is idempotent; concurrent import uses same coverage revision; rotated commit publishes nothing. Outside rows untouched. |
| L07 / HF-3 / `kucoin_import` | Derivation group aggregates UTC second; fee leg shared by multiple symbols; restored/user-edited row; ambiguous overlap | Promote whole group only when every contributing source row is covered, across all symbols. Otherwise CSV stays visible and corresponding API stays staged with explicit gap. Never delete half a group, match aggregate by amount alone, or overwrite protected rows. Completed disjoint recent unit may publish while old unit remains incomplete. |
| L08 / HF-3 / `kucoin_import`, `tracker_sync` | Persisted classic fills cutoff, classic API history, retained import_rows whose HF derivations were suppressed; new HF gap; unchanged CSV re-upload | Versioned migration archives cutoff and disables legacy unbounded fills policy for HF-enabled history. Both store and takeover use account/source/family/symbol/interval coverage, including resumed classic jobs. Rebuild from all retained rows at affected seconds even if no source row is new; each missing trade/fee appears once. Exact holdings/cost basis; classic rows and other classes unchanged. |
| L09 / HF-3 / `kucoin_import`, `tracker_sync` | Migration retry/crash, known bounded classic evidence, uncertain attribution, missing retained sources, API completion after backfill and later re-upload | Only proven bounded classic coverage converts; cutoff/earliest API timestamp/last_synced_at alone prove no interval or HF coverage. Preserve existing rows and stage uncertain overlap. Report missing-source gaps; known disjoint HF gaps allow CSV backfill. Atomic group rebuild/receipt resumes safely; current credential fences do not erase historical account coverage. Switching mode back cannot reactivate cutoff. |
| L10 / HF-3 / `kucoin_import`, `tracker_sync`, tracker gap tests | KuCoin export via existing `kucoin` format; first and last imported source-row timestamps; outside intervals | Ruling: coverage is the export's first-to-last-row period; outside remains a gap. Use existing tracker “Import history” line, no new prompt/copy. Do not treat a partial API response as an export baseline. Keep source timestamps/precision so rows at the period boundaries are included. |
| R01 / HF-4 / `placement`, `polling` | Lost placement reply followed by matching ID/clientOid lookups; mismatch/miss/error; restart and >1200 s | Exact original ID/clientOid, symbol, side/type and provenance required. Matching open/terminal lookup recovers one row. Missing/failed/mismatched lookup never proves non-placement, never clears intent, never repeats POST, even after timeout. absence_provable stays false. |
| R02 / HF-4 / `polling`, `reversal`, `handover` | Existing HF transaction, no placement intent; ages just below/at/above 2/7/14 days; positive old resting order; missing lookups or partial fills | Attempt positive lookup even for old resting orders. No fourteen-day abandonment and no forced Closed on fallback fills. Missing/truncated data retains known amounts and cancellation/reserve obligations. Never infer terminal zero, absence or complete totals from age. |
| R03 / HF-4 / `polling`, `reversal`, CLI tests | R02 + retained terminal observation or versioned operator evidence bundle; filled sell, cancelled partial buy, explicit zero fill | `reconcile-order <transaction_id> --evidence <file>` settles the existing row without an intent, new row or placement POST. Validate account/key provenance, raw ID, immutable symbol/side/HF family, terminal state/time, cumulative gross base/quote, fee currencies and evidence hash/provenance. Full execution list needs documented full-order coverage and deduplicated IDs. Store evidence/receipt without credentials. |
| R04 / HF-4 / `polling`, `reversal`, CLI tests | Missing terminal proof, partial totals, trade-only CSV, cancel acknowledgment, wrong account/order/symbol, conflicting/decreasing amounts or fees, stale/replacement credentials | No financial writes or obligation removal. Explain missing evidence and direct existing-row resolution to reconcile-order, never intent-only resolve-placement. Explicit zero totals required for proven zero fill. |
| R05 / HF-4 / `polling`, `reversal` | Valid R03; crash/retry, failed commit, two simultaneous resolvers or poll/resolver; differing evidence after settlement | Under credential/ownership fence re-read row/receipt. Atomically update final state, both executions, price/fees, cancel removal, reserve/proceeds, evidence and activity. Preserve IDs/links/origin. Exact replay no-op; conflict is visible. Reserve/proceeds credited once, no second fill/row/POST; failed commit retains every obligation. |
| R06 / HF-4 / `handover`, `placement` | Takeover with unresolved HF intents/orders/cancels; shutdown >7 days; current mode/credentials changed | Reconcile original family/account first; stop new spending until obligations resolve. Original-family positive reads allowed only with verified provenance. Mode change never redirects recovery; edited pair/replacement account never recovers an old intent. |
| R07 / HF-4 / `handover`, `polling`, `reversal` | Pre-HF Rust classic row versus adopted Rails row without provenance; candidate classic/HF endpoint reads | Pre-HF Rust rows stay known classic. Adopted row needs exact positive identity reconciliation; conflicting/missing evidence blocks it. Never infer HF from current mode or route cancel by current mode. Persist reconciled origin. |
| R08 / HF-4 / `handover` | Handback of HF-dependent bot, unresolved HF order/intent/cancel, UTA/unknown routing; concurrent ownership change | Rust refuses before and inside ownership transaction. Classic-only handback unchanged. No Rails patch. |
| F01 / HF-5 / `kucoin_hf`, `grid-kucoin-hf`, key-check tests | Factory/engine/sync/key validation connect completed HF paths | All prior rows green through real construction (including alpaca factory path), shared resolver and selected balance account. HF trading key checks require Spot permission; General-only fails. Failed reads preserve balances. No runtime opt-in or live-money gate. |
| F02 / HF-5 / `parity`, `sync_parity`, `kucoin`, `kucoin_import` | Classic wrapper, original fixtures and source guards, full HF replay matrix | All 172 grid-kucoin cases, signing/sizing/status vectors, key verdicts and KuCoin ledger/balance scenarios unchanged. Validate each named HF divergence narrowly in its actual replay result. Preserve D-KUCOIN-TRANSFER-PIN and D-KUCOIN-ZERO-FILL-FUNDS. |
| F03 / HF-5 / tracker gap tests | HF reading is connected | Remove the sentence claiming HF trades cannot be read, only when reading ships; use existing gap line and copy. HF-0 does not edit Rails/locales. |

## Mutation review and divergence mapping

| Deliberate implementation mutation | Rows that must fail |
|---|---|
| Default unknown mode to classic or reuse stale detection | M01–M04 |
| Read isActive instead of active; default missing flag to false | O04–O06 |
| Omit symbol/DELETE query from signature | O03, O07–O08 |
| Clear an ambiguous intent or repeat placement POST | O02, R01, R06 |
| Use tradeId as cursor or round integer IDs through f64 | L01–L02 |
| Treat short/clamped pages as complete or skip retention on resume | L01–L03 |
| Abandon old HF order or close partial fallback fills | R02–R04 |
| Resolve an existing row using intent-only resolve-placement | R03–R04 |
| Settle reserve twice or remove cancel barrier outside settlement transaction | R05 |
| Publish staged partial pages or delete outside covered interval | L05–L07 |
| Reuse classic fills cutoff or rebuild only newly inserted import_rows | L08–L09 |

D-KUCOIN-HF-MODE is validated by M01–M05 and F01;
D-KUCOIN-HF-ORDERS by O01–O08 and R01;
D-KUCOIN-HF-RETENTION by R01–R08;
D-KUCOIN-HF-LEDGER by L01–L10. These are future behavior divergences;
HF-0 introduces none into production. The matrix supplies the review checklist,
not a claim that those validators already exist.
