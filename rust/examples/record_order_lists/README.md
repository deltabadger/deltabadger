# Order-list recorder (A28-3b/3c fixtures)

Records each exchange's live order-list answers. It scrubs them and writes them to
`rust/tests/fixtures/order_lists/<venue>/` (PLAN-A28-fixtures.md). The tool only reads. It runs by hand, on your own
account, and never in a test or CI. It never commits: you review and commit.

## 1. A key that can only read

Create a fresh API key per exchange with **read/view permission only**: no trading and no withdrawal. Keep it in a
file outside the repo, for example `~/recorder-keys/kraken.env`:

```
RECORDER_KEY=...
RECORDER_SECRET=...
RECORDER_PASSPHRASE=...   # KuCoin and Bitget only
```

- **Coinbase, Gemini, KuCoin:** the tool reads the key's permissions first and refuses one that can trade or withdraw.
  Gemini needs an **Auditor** key. KuCoin needs **General** only.
- **The others:** the tool cannot tell, so it warns and goes on. Make sure the key is read-only.
- **Alpaca:** keys always trade. Use a **paper** account (`--paper`) unless you need live answers.
- **Hyperliquid:** the reads are public. `RECORDER_KEY` is your **wallet address**, and `RECORDER_SECRET` must not be
  set.

Keys are never arguments. The tool refuses any argument naming a key, secret or passphrase.

## 2. The test orders (by hand, on the exchange, before the run)

Use a cheap pair, for example BTC against the exchange's stablecoin, or XRPUSDT, with the smallest size the exchange
allows:

1. **one resting order:** a limit buy far below the market, left open;
2. **one cancelled order:** a limit buy far below the market, then cancelled;
3. **one filled order:** a small market buy.

Note a window that holds all three: `--since` a few minutes before the first and `--until` after the last. The **day
before `--since` must hold no orders on that symbol**, because the tool records `[since − 2 d, since − 1 d]` as the
empty case.

## 3. The command per exchange

Run from `rust/`. The symbol is in the exchange's own spelling.

```
set -a; . ~/recorder-keys/<venue>.env; set +a
cargo run --example record_order_lists -- kraken      --since 2026-11-06T10:00:00Z --until 2026-11-06T11:00:00Z
cargo run --example record_order_lists -- kucoin      --since … --until … --symbol BTC-USDT
cargo run --example record_order_lists -- coinbase    --since … --until …
cargo run --example record_order_lists -- alpaca      --since … --until … --paper
cargo run --example record_order_lists -- bitget      --since … --until … --symbol BTCUSDT
cargo run --example record_order_lists -- mexc        --since … --until … --symbol BTCUSDT
cargo run --example record_order_lists -- gemini      --since … --until … --symbol btcusd
cargo run --example record_order_lists -- bitvavo     --since … --until … --symbol BTC-EUR
cargo run --example record_order_lists -- bingx       --since … --until … --symbol BTC-USDT
cargo run --example record_order_lists -- bitrue      --since … --until … --symbol XRPUSDT
cargo run --example record_order_lists -- hyperliquid --since … --until …
unset RECORDER_KEY RECORDER_SECRET RECORDER_PASSPHRASE
```

One run records a venue's whole set, so the same id gets the same fake in every file. Each history read is recorded
three times:

- the full window;
- one page (`limit` 1 or the venue's page-size param);
- the empty window.

Open orders are recorded once.

**Not recorded:**

- Kraken and KuCoin-classic page boundaries: their smallest page is 50 and 10 orders, so a boundary needs more test
  orders than that.
- Gemini's empty case: its history read takes a start time only.
- Hyperliquid's window: `historicalOrders` takes none; `userFillsByTime` reads from `--since` on (no end, so no empty case).

**The run aborts and writes nothing** when:

- a request fails or answers non-2xx;
- the envelope is not a success;
- the full history lists fewer than 2 orders on every history read;
- the page case does not list exactly 1;
- the empty case lists any;
- the scrub or the leak check fails.

The message names the field path, never a value. A history read whose full window lists fewer than 2 orders (for
example KuCoin classic on an HF account) keeps its full file and drops its page and empty files. The manifest names
these under `not_recorded`.

## 4. Check the scrub, then commit

The tool prints every field it changed (`replaced`, `shifted`, `key-replaced`, with counts). The same list is saved
under `scrubbed` in `manifest.json`.

- **Replaced:** order ids, client ids, account ids, cursors, amounts and prices are replaced by fakes of the same shape.
  An all-zero amount (`"0.00000000"`) and an empty string are kept.
- **Kept:** enum fields such as status, side, type and symbol, when they pass a shape check.
- **Shifted:** times move back by one random whole-day offset, so only their date part changes.

Then:

1. Read the printed list. A field you need for a parser that was replaced goes into `KEEP` in `recorder.rs`, in a
   reviewed edit.
2. From the exchange's UI, copy your three order ids and one fill amount, then grep for each:
   `grep -rF '<value>' tests/fixtures/order_lists/<venue>/` must find nothing.
3. Grep for your key too, and for your Hyperliquid address.
4. `git diff --stat` and `git add tests/fixtures/order_lists/<venue>`, then commit by hand.

Revoke the read-only key when you are done.

## 5. KuCoin Unified Trading Account (`kucoin-uta`, PLAN-841 U5-0)

One spare KuCoin account (not the canary), read in three phases around its switch to UTA. All three share one scrub:
the same id gets the same fake, times shift by the same days and every amount is multiplied by the same factor (prices
are kept, so value / size still gives them). The state file holding that scrub maps fakes back to originals, so it
lives beside the keys and the tool refuses a path inside the repo.

Two keys on the account: **General** only for this tool (it refuses any other), **General + Unified** for the bot.

```
set -a; . ~/recorder-keys/kucoin-uta.env; set +a      # the General key
export RECORDER_STATE=~/recorder-keys/kucoin-uta.state.json
W=(--since 2026-11-21T08:00:00Z --until 2026-11-21T20:00:00Z --symbol BTC-USDT)
cargo run --example record_order_lists -- kucoin-uta --phase before   "${W[@]}"   # Classic/HF, a known fill in the window
#   switch the account to UTA on KuCoin, before moving any funds
cargo run --example record_order_lists -- kucoin-uta --phase switched "${W[@]}"
#   move the funds to the unified account, run the bot's market buy and the cancelled limit order
cargo run --example record_order_lists -- kucoin-uta --phase traded   "${W[@]}" --order <filled id> --order <cancelled id>
# next day, after 09:00 UTC+8: Account History export for the session's days
cargo run --example record_order_lists -- kucoin-uta-csv --file ~/Downloads/<export>.csv
unset RECORDER_KEY RECORDER_SECRET RECORDER_PASSPHRASE
```

The window is at most 7 days and holds the known fill. Read the cancelled order in the traded phase within two hours:
after that KuCoin no longer answers for an unfilled cancelled order.

Each phase writes `tests/fixtures/order_lists/kucoin_uta/<phase>/`, and the export goes to `kucoin_uta/export/`.
Execution and fill reads are recorded page by page (`name.1.json`, `name.2.json` …): `lastId` walks through the empty
last page, classic `currentPage` walks through `totalPage`, as the sync reads them.
Classic and HF routes on a UTA may refuse. A refusal is recorded as that route's answer (`"refused": true` in the
manifest) and listed in the report. A throttle, a 5xx, or a refusal on a UTA route the adapter reads aborts the run,
and nothing is written. The run also aborts when the mode doesn't match the phase, or when the before phase finds no
fill on `--symbol`. For each `--order`, the tool runs the adapter's own evidence check: detail plus executions,
Σ size = `filledSize`. If the scrub would break that check, nothing is written. If the raw answers already fail it,
the report names the order: that is U5-1's first failing test.

Delete the state file when the session's recordings are committed.
