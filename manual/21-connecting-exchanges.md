# Connecting exchanges

The Tracker reads your exchange accounts and turns their history into one portfolio. Everything under **Tracker** — the [overview](22-portfolio-overview.md), the [record](23-transactions-and-positions.md), the [tax reports](25-crypto-tax-report.md) — is built from what it fetches here.

## Adding an exchange

1. Open **Tracker** and press **+**.
2. Type the exchange name and pick it. The list shows each exchange's maker and taker fee.
3. Paste an API key with read permission and press **Connect**. The steps for creating one are next to the form; see [API keys](28-api-keys.md) for what each exchange requires.

An exchange already connected for a bot needs no second key: a trading key can read, so the Tracker uses it and skips the form. If that trading key has stopped working, the Tracker asks for a read-only key of its own rather than failing with the bots.

A rejected key shows **Incorrect API key permissions**. **Failed to validate API key permissions** means the exchange could not be reached to check it — try again in a moment. A new key starts fetching your transaction history right away; press **Sync** to load balances (the holdings card says **Click Sync to load your portfolio.** until you do). A reused trading key also needs **Sync**.

## Sync

**Sync** (the arrows button at the right of the bar) fetches your transaction history — trades, deposits, withdrawals, rewards — and your current balances from every connected exchange. Progress shows per exchange as **Importing data from …**; afterwards, withdrawals are matched to the deposits they became (see [Transactions and positions](23-transactions-and-positions.md)) and every figure is recalculated. You rarely need to press it: history and balances are fetched once a day on their own, and a bot placing an order syncs its exchange right away.

**Synced … ago** is the age of the balances. It turns amber when prices are older than the balances: the quantities are current, the money beside them is not.

## Exchange filter

With more than one exchange connected (or one whose key has stopped working), a switch appears above the chart: **All** or one exchange. It scopes the whole page — chart, figures, holdings, positions and transactions. A retired exchange stays in the list for the history it contributed but cannot be reconnected. An exchange whose key was removed or stopped working keeps its place, but its chip opens the key form instead of filtering.

## Sync warnings

A failed sync leaves a banner until the next sync succeeds: **Sync failed for … Transactions from there may be missing — reports generated now can be incomplete.** The message that arrives with the failure says what to do:

| Message | Cause |
|---|---|
| … is missing the permission needed to read your transaction history / your balances | The key is valid but lacks a permission. Fix it on the exchange, or create a new key. |
| … sync failed: … Please update the API key below. | The exchange rejected the key. It is marked invalid; enter a new one. |
| … is temporarily unavailable. | The exchange did not answer. Sync again in a few minutes. |
| … sync failed: … Please try again. | Any other error, with the exchange's own message. The key is not marked invalid. |
| Balances synced, but USD pricing is unavailable right now | Balances are fresh; the chart keeps the last known prices until pricing returns. |

The **Holdings** card names any asset the app could not price: **Pricing unavailable for: …**. Pricing needs market data — see [Market data](35-market-data.md).
