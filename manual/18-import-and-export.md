# Import and export

Exchanges do not always hand over their whole history, and a history is worth keeping outside the app. **Import** fills the gaps from a CSV file; **Export** writes the ledger out as one.

## Import

**Import** on the Tracker bar opens a dialog with a **Format** switch and a drop zone. Drop the file, or click to choose it — the import starts as soon as the file is picked. Duplicates are skipped, so importing the same file twice changes nothing, and a file the reader cannot make sense of is refused rather than half-imported. An import is attached to an exchange account, so connect the exchange first (see [Connecting exchanges](21-connecting-exchanges.md)).

**Deltabadger CSV** is the app's own format, the same file **Export** writes. Every row names its exchange, so one file can carry several accounts; rows for an exchange you have not connected are left out; if nothing else in the file was new, the dialog lists them: **No account connected for: …**.

| Column | Value |
|---|---|
| `date` | Time in UTC, e.g. `2024-03-01T14:05:00Z` |
| `type` | One of `buy`, `sell`, `swap_in`, `swap_out`, `deposit`, `withdrawal`, `staking_reward`, `lending_interest`, `airdrop`, `mining`, `fee`, `other_income`, `lost`, `withholding_tax`, `return_of_capital`, `adjustment`, `unsupported_activity` |
| `base_currency`, `base_amount` | The asset and quantity that moved |
| `quote_currency`, `quote_amount` | What was paid or received for it, if any |
| `fee_currency`, `fee_amount` | The fee, if any |
| `exchange` | The exchange identifier as it appears in an export, e.g. `binance`, `kraken` |
| `tx_id` | The exchange's own id, used to recognise the row on re-import; a row without one is matched on exchange, type, asset, amount and time |
| `group_id` | Ties the two legs of a swap together |
| `description` | Free text |

A row with an unknown `type` is skipped, and listed when nothing else was imported: **Not imported, unknown operations: …**.

**Binance CSV** reads the transaction history Binance exports from its own site, which reaches further back than the Tracker can fetch through the API. Upload it under the name Binance gave it: the rows carry no time zone, and the file name does — a renamed file is refused. Trades, converts, dust sweeps, deposits, withdrawals, rewards and interest are recognised; moves between your own Binance wallets are ignored on purpose; anything else is listed as unknown. If both Binance and Binance.US are connected, the dialog asks which account the file belongs to.

When rows land, the dialog closes and the page reloads with **Imported N transactions.** If every row was already present it says so instead.

## Export

**Export** downloads `deltabadger-transactions-<date>.csv` with the columns above, oldest row first. It follows the exchange filter and the two date pickers under the record, and it is never limited to the rows shown on the page.

The same export is available from the **Tax Report** button: in the **Generate Report** dialog choose **All**, set **From** and **To**, and press **Download**. Without market data that dialog asks for a CoinGecko API key instead (see [Market data](35-market-data.md)); use **Export** on the bar. The rest of the dialog is described under [Crypto tax report](25-crypto-tax-report.md).
