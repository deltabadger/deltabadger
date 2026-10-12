# Rust run ownership and Rails parity

Rails keeps main's #526 lifecycle and placement behavior. Run generations, durable client-ID
intents, and immutable accepted-order attribution are Rust protocol data. The shared migration
adds only `transactions.run_identity`; Rails does not write it. Rust keeps intents in the existing
`bots.transient_data.rust_placement` key. There is no Rails `placement_intents` column or consumer.

Named stricter divergences:

| Name | Rails witness | Rust assertion |
| --- | --- | --- |
| `same_timestamp_restart` | A resumed claim with the same timestamps passes the old tick's fence. | `placement::divergence_same_timestamp_restart_fences_the_old_order` sends nothing and removes its undispatched intent. |
| `stale_tick_completion` | The old tick can transition the restarted run to waiting. | `placement::divergence_stale_tick_completion_cannot_write_over_a_new_run` refuses the write; `tick_basket` tests pre-claim and final-leg races. |
| `settings_held_until_reconciliation` | A stopped bot accepts an amount edit with an unresolved commitment. | `web_actions::action_write::divergence_settings_held_until_reconciliation` refuses the edit and preserves the carry window and intent. |
| `accepted_order_recovery` | Ambiguous venue responses can leave Rails without a submitted row. | The existing named decision-parity divergences in `parity.rs` require Rust to retain the client-ID intent, reconcile the accepted order once, and hold further placement. `placement` tests preserve original run attribution across STOP and credential rotation. |
| `in_flight_stop_notice` | Rails' STOP response has no in-flight notice. | Browser and MCP STOP responses warn when Rust retains an unresolved intent or a submitted transaction has unknown/open external status. This changes no order or lifecycle decision. |

`fixtures/run_ownership.json` records the first three witnesses using unchanged Rails code in a
clean archive of main. `script/rust/record_run_ownership.rb` runs only synthetic data, with network
access fenced. Tests pin the Rails sources and assert both the recorded Rails outcome and Rust's
stricter outcome. Existing recovery comparisons remain in `parity.rs`.

Business-row parity omits only Rust's `run_generation`, `tick_generation`, and transaction
`run_identity` metadata, alongside the pre-existing Rust intent/wait/mail keys. Dedicated protocol
tests assert their values and cleanup. Settings, carry, transaction amounts and times, status,
activity, and requests remain compared.

The production placement adapter is Alpaca paper. It performs no blocking venue preflight or
session acquisition between dispatch reservation and HTTP submission. The generation check runs
after acquiring SQLite's write lock; `ownership_rechecks_generation_after_waiting_for_the_dispatch_write_lock`
proves a STOP committed during that wait prevents transmission. Future adapters with a blocking
pre-send step must recheck after that step. Once HTTP submission begins, STOP cannot retract the
request: it commits promptly, while the reply is recorded in a separate short transaction.

Fenced cleanup is an atomic compare-and-remove of the matching **undispatched** intent. A different
intent or one already marked dispatched remains available for reconciliation. No database write
transaction spans venue I/O.

Legacy intents written by main have no `run_identity`. Recovery and operator resolution preserve
Rails' ambiguous-placement checkpoint wait for these intents, using the current schedule under
the settlement write lock. Missing attribution never grants dispatch ownership. The `run` upgrade
test constructs main's persisted intent format, proves absence, restarts the engine, and asserts
no purchase until strictly after the next checkpoint.
