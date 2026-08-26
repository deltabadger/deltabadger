# Index changes

When a coin drops out of an [index bot](17-index-bot.md)'s composition, or you remove an asset from a [portfolio bot](15-portfolio-bot.md), the bot still holds it. It is listed, you decide when to sell it, and the proceeds can go back into the basket.

## The table

Under the holdings table on the bot page a second table appears, headed **Left the index** (index bot) or **Removed from portfolio** (portfolio bot). Each row shows Amount, Avg. Price, Value and P/L for one asset, with a **Sell** button. Nothing in it is sold automatically: the bot stops buying the asset and [rebalancing](16-rebalancing.md) ignores it. A holding too small to sell at the exchange's minimum order size is not listed. A coin that re-enters the index moves back to the main table.

## Sell

Press **Sell** on a row. The confirmation reads "Sell this position? It is closed at market price." The whole position is sold with a market order, never more than is actually on the exchange, and the orders table records "Selling an asset that left the index". Positions are sold one at a time, because each sale is a separate taxable disposal.

A sale is refused while a rebalance, another sale or a redeploy is still in progress, while the market is closed (stocks), and on an archived bot. If FeeCutter left an open limit buy for that asset, it is skipped — "An asset that left the index could not be sold — left in place" — until that order settles or is cancelled.

## Realised P/L

Once a sale has realised a gain or loss, the statistics panel gains **Realised P/L**: what the sales brought in minus what the sold assets cost. Its tooltip reads "From selling assets removed from the portfolio. Rebalancing is not counted." Selling does not move the bot's total P/L at that moment — the holding's value becomes cash, which **Portfolio Value** still counts.

## Redeploy

The proceeds stay as cash on the exchange. At the bottom of the statistics panel the bot asks "Redeploy 250.00 USD?" with **Yes** and **No**.

- **Yes** buys back into the basket at market, spread across the assets below their target weight, even while the bot is stopped. A share too small for the exchange minimum is added to the most underweight asset instead.
- **No** takes that amount off the table for good; proceeds of later sales are offered on their own. The cash stays on the exchange, and regular contributions count it before new money.

The prompt is hidden while a sale, rebalance or redeploy is still working, and for amounts below the smallest minimum order size in the basket.

## Unconfirmed orders

If the outcome of a sale or a redeploy order cannot be confirmed, the panel shows "Sale unconfirmed — check your exchange" or "Redeploy unconfirmed — check your exchange" with a **Clear** button, and the bot places no orders until you clear it.

> **Note:** Check the exchange before pressing **Clear**. Clearing while the order is live lets the bot trade on top of it — selling coins twice or spending the same cash twice. If the order is still open, the app refuses: "Order … is still open on your exchange. Wait for it to settle first."
