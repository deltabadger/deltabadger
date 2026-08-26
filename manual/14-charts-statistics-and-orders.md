# Charts, statistics and orders

The bot page shows what the bot has done: a chart of the position over time, a statistics panel with the holdings, and a log of orders and events. The chart appears after the bot's first order attempt.

## Chart

The chart draws the portfolio value against the total invested, marked at the market price. The headline shows the date, the P/L and the percentage; move along the curve to read any point.

A switch above the plot toggles between **Value** and **P/L**. **Value** plots both curves; it always climbs for a DCA bot because the money going in climbs. **P/L** makes the invested line the zero line and plots the distance from it, so the shape is performance alone.

Point at a row of the holdings table to draw that asset alone; click the row to pin it so you can read along its curve, click again to let go. With **Hide balances** on (see [Account settings](33-account-settings.md)) the chart shows percentages only.

## Statistics

**Total Invested** (**Total invested** on portfolio and index bots) and **Portfolio Value** are shown in the bot's quote currency. Portfolio and index bots add **Realised P/L** once something has been sold from the basket: it comes from selling assets removed from the portfolio, and rebalancing is not counted, see [Index changes](18-index-changes.md).

The holdings table lists each asset with **Amount**, **Avg. Price**, **Value** and **P/L**. If the exchange cannot be reached the panel says so: **The exchange API does not respond at the moment. Calculations are based on the last data available.**

## Orders

The log combines orders and bot events. Tabs appear once there is more than one kind of row:

| Tab | Rows |
|---|---|
| **All** | Everything as sentences: **Bought 0.001 BTC for 100 USD**, **Bot started**, **Paused: price limit not met**, **Rebalancing — selling the overweight asset** … |
| **Transactions** | Filled orders with **Date**, **Amount**, **Value** and **Price** |
| **Scheduled** | Open orders, that is limit orders not yet filled |
| **Other** | Cancelled, skipped and failed orders |

An open order has a **Cancel** button that cancels it on the exchange. A row marked **Skipped (below minimum)** was under the exchange minimum; the amount is carried into the next order, see [Amount and interval](09-amount-and-interval.md). Older rows load on their own below the newest ones.

## Export and Import

**Export** refreshes the open orders and downloads `orders.csv` with every filled order: `Timestamp`, `Order ID`, `Type`, `Side`, `Amount`, `Value`, `Price`, `Base Asset`, `Quote Asset`, `Status`.

**Import** takes a CSV exported from Deltabadger and adds its orders to this bot, for example after recreating a bot. Only rows with the status `Closed` are imported, only those whose currencies match the bot's, and rows already imported into this bot are skipped. The result is reported as **Successfully imported N order(s)**, **No new orders to import …**, **Currency mismatch …** or **Invalid CSV format. Please use a file exported from this application.** Imported orders count in the statistics and cannot be cancelled.
