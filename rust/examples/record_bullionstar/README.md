# BullionStar recorder (PLAN-bullionstar.md slice 0)

Records BullionStar's live answers, scrubs the account data out of them, and writes them to
`rust/tests/fixtures/bullionstar/<run>/`. It also prints observations for the plan's open facts. Run it by hand on
your own account, never in a test or in CI. It never commits anything: you review the output and commit it yourself.

What the runs settle, and what they leave to you:

| Fact | Settled by a run | Left to you |
|---|---|---|
| F1 key and whitelist | probe, both labels | |
| F2 SVF balance array | its length per currency | what each element means: compare with the account page |
| F3 catalogue | read | |
| F4 method ids | read | |
| F5 lock lifetime, single use | orders (`--expiry` for an expired token) | |
| F6 refusals | orders: repeated token, funds, holding, `--unavailable`, `--wrong-pin` | a real order's refusal (G-LIVE) |
| F7 test orders | orders: id, lookups, balances around each one | |
| F8 order status | the status right after a test order | later transitions: the G-LIVE buy |
| F9 minimums, sell quote | the sell lock's quote | minimums per product: the dealer's pages |
| F10 hours | | run `orders` once at a weekend or night hour |
| F11 rate limit | 429 count and headers, if any | |
| F12 account identity | orders | |
| F13 costs | orders: 1 and 5 grams, 1 bar | a limit on unused locks |

**Every order it sends has `testOrder: true`.** The guard inside the transport refuses any order without it, any
call that is not on its allow-list, and the PIN anywhere except an order body.

## 1. The key

In your BullionStar account, create an API key whose IP whitelist includes the machine you run from. Keep the key and
PIN in a file outside the repo, for example `~/recorder-keys/bullionstar.env`:

```
BULLIONSTAR_KEY=...
BULLIONSTAR_PIN=...   # only orders mode sends it; every mode reads it when set, so the guard and leak check look for it
```

Keys are never arguments. The tool refuses an argument that names a key or token, or that holds the PIN. The key
is only ever sent to `api.bullionstar.com`.

## 2. The runs, in order

Run from `rust/`:

```
set -a; . ~/recorder-keys/bullionstar.env; set +a

# F1: run once from a whitelisted IP, then once from an IP that is not on the whitelist (a phone hotspot).
# The probe reads locations, one product page and one price with the key, without it, and with a wrong one:
cargo run --example record_bullionstar -- probe
cargo run --example record_bullionstar -- probe --label off_whitelist --location <id> --product <id>   # the ids the first probe printed

# F2-F4, F9, F11: locations, the whole catalogue with every product's detail, prices in 12 currencies,
# payment/shipping/sell-payment methods, SVF and vault holdings. Several minutes (250 ms between calls);
# --details N reads only N details per location.
cargo run --example record_bullionstar -- read

# F5-F8, F12, F13: pick the ids from the read's printed observations (the SVF payment method, the vault-storage
# shipping method and the SVF sell-payment method of one location and currency), a BSP gram you hold, and a bar.
cargo run --example record_bullionstar -- orders --location 1 --currency USD --gram <id> --bar <id> \
    --payment <id> --shipping <id> --sell-payment <id> [--unavailable <id>] [--wrong-pin] [--expiry]

unset BULLIONSTAR_KEY BULLIONSTAR_PIN
```

The orders run, in order:

1. reads the vault and SVF balances;
2. takes buy locks for 5 grams and for one bar (only the costs are recorded, F13);
3. takes a lock for 1 gram and places it as a test order. **Unless the dealer acknowledges it with an order id (a
   mistyped PIN, too little SVF), the run sends no further order and stops here**, so a wrong PIN is never sent
   twice. It also stops if a test order moved a balance;
4. reads the test order back by id and by reference, then the balances; then sends the same token again
   (repeated-token refusal);
5. takes a vault-sell lock for 1 gram, places it as a test order, and reads the balances again;
6. places test orders that should be refused: 1000 bars (insufficient funds) and a sale above your holding;
7. `--unavailable`: a product the read showed as not available for purchase;

   After every order it sends (steps 3-9), the run reads the balances again. It stops before the next order if a
   balance moved, a read failed, or the answer was neither an acknowledgement nor a definite refusal (a 4xx);
8. `--wrong-pin`: **one** test order with a wrong PIN. It is off by default because the dealer may lock the account
   after a few wrong PINs;
9. `--expiry`: waits for a lock to expire (20 minutes at most), then places it as a test order;
10. reads the balances again. The report says, for each test order and for the whole run, whether a balance moved
    (F7).

A refusal by the dealer is recorded and the run goes on. A network failure, or a call the guard refuses, stops the
run and writes nothing. A refusal to a test order may differ from a refusal to a real order; the G-LIVE buy
(D-BS-LIVE) confirms them.

## 3. What is written, what is printed

- **Catalogue answers** (locations, products, details, prices, method lists) that succeeded are written as they came.
  They are the dealer's public data. A catalogue call that failed is scrubbed like an account answer.
- **Account answers** (holdings, locks, orders, refusals, the probe's 401/403) go through the shared scrubber.
  Ids, references, tokens, serial numbers, names, amounts and balances are replaced by fakes of the same shape,
  and times move back by a random whole number of days. Text in a non-Latin script becomes `x` first. These fields
  are kept: enum fields (`status`, `statusString`, `type`, `metal`, `currency`, `locationId`, ...), booleans (`paid`,
  `stored`), and an error's `message` (the adapter matches on it, F6) when it is short ASCII text with no digit, no
  `@` and no long token. Any other message is scrubbed, and withheld from the printed lines. The report lists every
  kept message: check that none names a person before you commit.
- **The leak check** runs before anything is written. It looks for the key anywhere, the PIN as a whole value or a
  `pin` field, and every replaced value in the scrubbed files. A hit writes nothing and names the field's path.
- **The observations** are printed only and never written. They hold the dealer's own values (method ids and titles,
  lock costs, lifetimes), status codes, and error messages. Paste the F lines into the plan's findings.

## 4. Check, then commit

1. Read the printed `scrubbed` list in `manifest.json`.
2. Grep the output for your account id, an order reference, a serial number, and your key:
   `grep -rF '<value>' tests/fixtures/bullionstar/` must find nothing.
3. `git add tests/fixtures/bullionstar` and commit by hand.

When you are done, revoke the key or remove the recording machine's IP from its whitelist.
