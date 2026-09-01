# Transactions and positions

The record, below the holdings card, lists every transaction the Tracker knows and the positions they add up to. The P/L figures and the tax reports are computed from it, so this is where you correct it.

## Positions and Transactions

The switch at the top of the record chooses the view.

**Positions** shows one row per open position, closed round-trip or cash balance, with its dates, invested amount, average buy price, current or sold price, P/L and holding period. An open row is marked at today's price, a closed one at what it sold for. Status is **Open**, **Win**, **Loss**, **Cash**, or **Closed** — a round-trip whose cost was assumed somewhere along the way, so no win or loss is claimed. A status filter above the table narrows the rows. A coin the exchange has stopped reporting keeps its row; that gap is a finding (below).

**Transactions** is the ledger: **Date**, exchange, **Type**, **Asset**, **Amount**, **Price**, **Value**, **Fee** and **Bot** (a link to the bot that placed the order). Amount, price and fee are the exchange's own figures; **Value** is in your display currency. A type filter and two date pickers narrow the table; the dates also scope the export. The table shows the latest 200 rows; **Export** always contains all of them (see [Import and export](24-import-and-export.md)).

Types: **Buy**, **Sell**, **Swap In**, **Swap Out**, **Deposit**, **Withdrawal**, **Staking**, **Interest**, **Airdrop**, **Mining**, **Fee**, **Income**, **Lost**, **Withholding tax**, **Return of capital**, **Adjustment**, **Other activity**. A withdrawal and deposit linked as a transfer both show **Transfer** instead.

## Transfers

A withdrawal on its own is coins leaving the portfolio: they take their cost with them and reduce **Total invested**. Linked to the deposit they became elsewhere, the pair is a transfer between your own accounts — the coins keep their cost basis and holding period, nothing is realised, and the tax report skips both rows.

Each sync links the obvious cases: same asset, a deposit within 72 hours of the withdrawal, at most 2% smaller. For the rest, hover a **Deposit** or **Withdrawal** row and press **Mark as transfer**; the other leg must be the single match within 14 days, or nothing is linked. **Not a transfer** undoes a link, and later syncs leave the pair alone.

## Value

For a trade, **Value** is what the exchange reported. For a row it did not price — a reward, an airdrop, a deposit — it is the app's own price for that day, shown as a placeholder in a box. Type a figure in your display currency to replace it; clear the box to go back. Hover the box to see whose figure it is.

## Findings and Fix

When the ledger cannot stand behind a holding, a panel above the holdings card says so: **N holdings the ledger cannot vouch for**, one line per case — more left the account than ever arrived, the ledger and the exchange disagree about how much is held, a balance has no history behind it, or part of it arrived without a price. Such a holding shows **—** for its P/L instead of a wrong number.

**Fix** proposes the missing entry. For an opening balance it works out the quantity and date and asks only how the coins arrived: **I earned it** (income at that day's price, which becomes its cost) or **I bought or transferred it** (a cost box, prefilled with that day's market price when one is known — change it, or empty it to leave the cost unknown: the quantity reconciles, the P/L stays blank). For a disposal the exchange no longer reports, it records the coins as having left, realising nothing. **Record this entry** writes it into your transactions as your own entry.
