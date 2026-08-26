# Rebalancing

The **Rebalance** option keeps a [portfolio bot](15-portfolio-bot.md) or an [index bot](17-index-bot.md) close to its target weights by swapping between the assets it holds. It is separate from the DCA contribution: it runs on its own clock, spends no new money, and keeps working while the schedule is stopped.

## Turning it on

The last toggle in the bot's settings reads "Rebalance when an asset drifts more than **5** % from its target". The number is the drift threshold in percentage points; the default is 5. Drift is the largest gap between any one asset's current share of the portfolio's value and its target — an asset meant to be 40% that is now 47% is 7 points off. It is measured on current value, not cost.

This toggle can be changed while the bot is running. While it is on, the settings show "Keeps running while the DCA schedule is stopped. Always uses market orders." and the live reading "The portfolio is 6.2 % off its target split", green once it exceeds the threshold. If the correcting trade would be below the exchange's minimum order size, it adds "— too small to trade at this exchange's minimum" and nothing happens until the drift grows.

## What a rebalance does

1. Sells the most overweight asset down to its target, at market.
2. Buys the most underweight asset with the proceeds, at market.

One order per check: the sell at one check, the buy at the next once the sell has filled, so a swap takes up to two checks and the proceeds sit as cash in between (still counted in **Portfolio Value**). With more than two assets each swap fixes one pair and the portfolio converges over several checks. Money the buy does not need goes to the next-most-underweight asset; too little to place is logged as "Too little left over to rebalance — leaving it as cash". The bot never sells more than is actually on the exchange.

Rebalance orders are always market orders, even with FeeCutter on, so expect taker fees on both legs. They are not contributions: they do not count toward **Total invested** or **Realised P/L**. The orders table shows them as "Rebalancing — selling the overweight asset" and "Rebalancing — buying back into the underweight asset". Assets that have left the composition are ignored — see [Index changes](18-index-changes.md).

## When it is checked

Every 4 hours, independently of the bot's interval, and also while the bot is paused. Archived bots are not checked; stocks are only traded while the market is open.

While a rebalance is in progress — sold, buy still owed — a scheduled contribution is skipped ("Contribution skipped — a rebalance is still in progress") and carried into the next one. The exchange cannot be changed until it completes, and on a portfolio bot the assets and allocations are locked too. Turning the toggle off mid-swap still lets the owed buy finish.

## If it halts

When the outcome of a rebalance order cannot be confirmed, the settings show "Rebalancing paused — check your exchange" with a **Resume** link, and the bot places no further orders.

> **Note:** Check the exchange for open rebalancing orders before pressing **Resume** — resuming on top of a live order can sell or buy the same amount twice. If an order is still open, the app refuses: "Order … is still open on your exchange. Wait for it to settle first."
