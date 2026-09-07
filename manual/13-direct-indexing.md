# Direct indexing

Direct indexing means owning the stocks of an index yourself, in your own brokerage account, instead of a fund that owns them for you. Deltabadger does it with an index bot: pick an index, pick how many of its constituents to hold, and the bot buys them at market-cap weights on your schedule, rebalances them, and lets you sell any single position on its own.

## ND100

**ND100** is Deltabadger's stock index: the 100 largest non-financial companies listed on the Nasdaq stock exchange, weighted by market cap and refreshed daily. A bot can hold all of them or trim the list with the slider — an ND100 bot cut to twenty names is called **ND20**. The allocation-flattening slider moves the weights from market cap toward equal weight.

ND100 trades on [Alpaca](21-supported-exchanges.md). Every constituent is bought as a fractional share, so the schedule works from small amounts; on a small contribution the lightest names wait until enough has accumulated for the exchange's minimum order, and the bot's log says which ones carried forward.

> [!NOTE]
> Nasdaq® and Nasdaq-100® are registered trademarks of Nasdaq, Inc. Deltabadger is not affiliated with, sponsored by or endorsed by Nasdaq, Inc. ND100 is built from the published membership of the Nasdaq-100 Index, obtained from a licensed market-data vendor, and weighted by that vendor's market capitalisations, normalised by Deltabadger. It is not the Nasdaq-100 Index® and does not use its weights.

## Selling a position

Every position in the bot's table has a **Sell** button. Selling closes the whole position at market; it is a taxable disposal, and the confirmation names exactly what is being sold. A remainder too small for the exchange to trade has no button.

## Index changes

Companies join and leave the index. A constituent that has left is moved to the **Left the index** table and is no longer bought; selling it is your call, from its own row, because it is a disposal with tax consequences. See [Portfolio rebalancing](12-portfolio-rebalancing.md).
