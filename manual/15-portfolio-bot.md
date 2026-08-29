# Portfolio bot

A portfolio bot invests one amount across a basket of 2–20 assets at weights you choose. Use it when you want one schedule and one budget for several assets instead of a bot per asset.

## Creating one

Press **New bot** and choose the **DCA / Rebalance** card. On the **Pick asset** step add two or more assets; the basket table under the search removes one with **×**. The rest of the wizard is the same as for one asset — see [Creating your first bot](08-creating-your-first-bot.md). All assets must trade against the same quote currency on one exchange; otherwise the wizard says "This combination of assets doesn't exist on any supported exchange." For stocks see [Stocks and ETFs](29-stocks-and-etfs.md).

The bot starts with equal weights and is named after its basket, for example "BTC, ETH, XRP + 3".

## Allocations

The main rule reads "Invest 100 USD / Week" above one slider per asset. Each slider is that asset's share in percent; the **Total** row shows the sum. The bot cannot start until the total is 100% — "Allocations add up to 95% — they must add up to 100% before the bot can start." Press **Normalize** to scale the sliders proportionally to 100%.

- **+ Add asset** searches for another asset the exchange lists against your quote currency (up to 20). It starts at 0%.
- **×** next to a slider removes the asset (at least 2 must remain).
- An asset at 0% stays in the portfolio and receives nothing from contributions; with **Rebalance** on, anything it still holds is sold down over the next checks.

Assets and allocations are locked while the bot is running and while a rebalance is in progress. The quote currency cannot change once the bot has placed orders.

## How a contribution is split

At every interval the bot values its holdings at current prices, adds the contribution, and works out each asset's target value at its weight. The money goes only to assets below target, in proportion to their shortfall. Nothing is sold; an asset far above its weight receives nothing until the others catch up, so contributions alone pull the portfolio toward its weights. An order below the exchange minimum is skipped and buffered into the next contribution — see [Amount and interval](09-amount-and-interval.md).

## Options

A portfolio bot has **Smart Intervals**, **FeeCutter** and **Starting time** — see [Order options](10-order-options.md) — and **Rebalance**, see [Rebalancing](16-rebalancing.md). It cannot switch to [selling](12-selling.md); that exists only on single-asset bots.

## Removed assets

Removing an asset you already hold does not sell it. It moves to the **Removed from portfolio** table, where you can sell it and redeploy the proceeds — see [Index changes](18-index-changes.md).

## Former two-asset bots

Two-asset "Rebalanced DCA" bots were converted into portfolio bots with two assets. Their history, open orders, trading conditions and schedule carried over unchanged; the only visible difference is that the allocation is now edited with the portfolio sliders, and a market-cap-weighted pair follows the stored market caps rather than a live ratio.
