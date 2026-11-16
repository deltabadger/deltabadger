# record_ibkr — IBKR answers as test fixtures

Records IBKR's live Web API answers, scrubbed, into `rust/tests/fixtures/ibkr/<read|paper>/`. The IBKR slices build against these recordings:
- A27-2: data-api C0 (`secdef_info`, conid resolution);
- A27-3: trading (`ibkr_orders.json` from paper answers, `REPLY_WHITELIST`);
- A27-5: the PA ledger;
- A27-6: splits.

The plan is `PLAN-A27-1-ibkr-recorder.md`.

It signs through the engine's own IBKR session (`venue::ibkr::Session`), so every recorded request is the one Deltabadger sends.

There are two modes:
- **read** reads only, on any account. It sends no order, reply or cancel; the tool refuses those paths in read mode.
- **paper** places, confirms and cancels orders. It refuses to start unless the login lists exactly one paper account (`DU…`/`DF…`). Every order, reply and cancel is checked again, at send time, against an account list read just before it.

Run it by hand on your own login. Never run it in a test or CI.

## 1. G1: a first-party consumer on your IBKR Pro login (for read mode)

IBKR keeps **one** consumer registration per username. If this username already backs a Deltabadger IBKR connection, reuse that connection's material (the same consumer key, access token, secret and key files). A new registration would replace it and break the connection.

1. Generate the keys in a directory **outside the repository**, for example `~/ibkr-live`:
   ```
   mkdir -m 700 ~/ibkr-live && cd ~/ibkr-live
   openssl genrsa -out private_signature.pem 2048
   openssl rsa -in private_signature.pem -pubout -out public_signature.pem
   openssl genrsa -out private_encryption.pem 2048
   openssl rsa -in private_encryption.pem -pubout -out public_encryption.pem
   openssl dhparam -out dhparam.pem 2048
   chmod 600 *.pem
   ```
2. Open IBKR's OAuth self-service portal in a private window and log in with the IBKR Pro username. Check that the address bar reads `ndcdyn.interactivebrokers.com` or `www.interactivebrokers.co.uk`: the upload works only there, and error 501 means you are on another server.
3. Set a consumer key of exactly 9 letters (A–Z). Upload `public_signature.pem`, `public_encryption.pem` and `dhparam.pem`.
4. Copy the access token and the access token secret. The portal shows them **once**.
5. Write the three values into files in the same dir: `consumer_key`, `access_token`, `access_token_secret`. Optionally add a `realm` file; the default is `limited_poa`. Then `chmod 600` them.

A new registration can take a while to activate. Until it does, the LST mint fails.

## 2. G3: a paper login (for paper mode)

A paper username registers separately: log in to the portal with the **paper** username and repeat steps 2–5. That gives its own access token and secret. The key files may be the same ones. Keep the result in its own dir, for example `~/ibkr-paper`. This follows IBKR's first-party docs and other users' reports. If the portal refuses the paper username, tell me.

Nothing else may use the login during a run: the tool takes over the brokerage session (`compete: true`).

## 3. Commands

From `rust/`:

```
IBKR_RECORDER_DIR=~/ibkr-live  cargo run --example record_ibkr -- read  --us AAPL,265598,US0378331005 --eu SYMBOL,CONID,ISIN
IBKR_RECORDER_DIR=~/ibkr-paper cargo run --example record_ibkr -- paper --stock SYMBOL,CONID,ISIN --limit PRICE [--untradable CONID]
```

- `--us` / `--eu` / `--stock` take `SYMBOL,CONID,ISIN` as IBKR spells them. The conid is in TWS or the Client Portal, under the contract's details.
- `--eu`: any liquid stock quoted in a non-USD currency (EUR), for example SAP. You need not hold it: its quote, history, rules and contract lookups are recorded either way. If you do hold one, the positions and PA answers also get a non-USD row.
- `--limit`: about half the stock's price. The limit buy must never fill; it is cancelled straight away.
- `--untradable`: optional. A conid IBKR does not trade (an index such as SPX, conid 416904) records the refusal of an order on it. Leave it out if unsure.
- Credentials are never arguments. An argument that mentions a key, secret, token or pem is refused.

## 4. What to have in place first

- **read**: on the live account, ideally at least one held position and one past trade. Then the positions, trades, order status and PA answers have rows. Empty answers are recorded too. The positions read records page 0 and page 1. Page 1 is normally empty: that is the page boundary, and it is also the recording's empty answer.
- **paper**: run during **US market hours**, on a cheap US stock (one share is bought and sold). The tool does the following:
  1. places a market buy, confirms every prompt, and polls the order until it is filled;
  2. places a far limit buy, reads its status, cancels it, and reads it again;
  3. places a market sell of the share, and polls it until it is filled;
  4. places the optional untradable order;
  5. reads the account.

  If an order is not filled within a minute, the run stops and writes nothing. A paper order may then still be open: cancel it in the portal.

## 5. Checking the scrub before committing

The tool writes nothing unless the scrub and the leak check both pass:
- no replaced value and no credential may appear anywhere in the output;
- account ids keep their `U`/`DU`/`DF` prefix, with the digits faked;
- order ids, cOIDs, reply and execution ids, amounts, prices, cash, positions and names are faked consistently across files;
- every free-text field (prompt texts, error messages, descriptions) becomes `[text]`;
- times move back by one whole number of weeks;
- kept as they are: conids, enums (status, side, order type, currency, field 6509, message ids) and, in successful contract-lookup answers only, the symbol, ISIN, exchange and contract rules;
- an unknown JSON key is replaced and counted under `key-replaced` in the report. Send me that list: a field that A27 needs gets added to the vocabulary, and you re-record.

Still check it yourself, before `git add`:

```
grep -rn -e 'U1234567' -e 'DU7654321' -e '<your consumer key>' -e '<an order id you know>' rust/tests/fixtures/ibkr/
```

Replace each with your own account ids, consumer key and a real order id. The command must print nothing.

The terminal also shows each confirmation prompt's text beside its message ids, so you can decide which ids to whitelist (A27-3). That text is never written to a file. The transport's failure lines omit response bodies. A rerun replaces the mode's whole set (another salt, another offset). The tool never edits `ibkr_orders.json` and never runs git.
