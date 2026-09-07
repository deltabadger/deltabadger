# Direct indexing

Direct indexing means owning the stocks of an index yourself, in your own brokerage account, instead of a fund that owns them for you. Deltabadger does it with an index bot: pick an index, pick how many of its constituents to hold, and the bot buys them at market-cap weights on your schedule, rebalances them, and lets you sell any single position on its own.

## ND100

**ND100** is Deltabadger's stock index: the 100 largest non-financial companies listed on the Nasdaq stock exchange, weighted by market cap and refreshed daily. A bot can hold all of them or trim the list with the slider — an ND100 bot cut to twenty names is called **ND20**. The allocation-flattening slider moves the weights from market cap toward equal weight.

ND100 trades on [Alpaca](21-supported-exchanges.md). Every constituent is bought as a fractional share, so the schedule works from small amounts; on a small contribution the lightest names wait until enough has accumulated for the exchange's minimum order, and the bot's log says which ones carried forward.

> [!NOTE]
> Nasdaq® and Nasdaq-100® are registered trademarks of Nasdaq, Inc. Deltabadger is not affiliated with, sponsored by or endorsed by Nasdaq, Inc. ND100 is built from the published membership of the Nasdaq-100 Index, obtained from a licensed market-data vendor, and weighted by that vendor's market capitalisations, normalised by Deltabadger. It is not the Nasdaq-100 Index® and does not use its weights.

## Selling a position

Every position in the bot's table has a **Sell** button. Selling closes the whole position at market; it is a taxable disposal, and the confirmation names exactly what is being sold. A remainder too small for the exchange to trade has no button.

The button turns **green** when the position is under water on this bot's own purchase lots — selling it then realises a loss you can set against gains. That is the moment direct indexing exists for: a fund cannot hand you the loss on one of its holdings, your own account can. The colour follows prices, so it changes.

The figure behind the colour is an estimate: the bot's own purchases, counted first-in-first-out in the bot's currency, one lot per order. The same asset bought elsewhere on the account, by another bot or by hand, is not in that arithmetic; the United Kingdom pools purchases at average cost and Ireland matches sales to purchases of the previous four weeks; and the tax report converts into your reporting currency. For a position that is barely under water those differences can flip the sign. The [tax report](20-broker-tax-report.md) over the whole account is the authority.

## Wash-sale window

Some tax systems disallow a loss if you buy the same asset back too soon: the United States (30 days), the United Kingdom (30 days, where the repurchase is matched against the sale instead of the pool) and Ireland (28 days). Pick your jurisdiction in the bot's settings and the bot enforces the window itself: after a sale at a loss it leaves that constituent out of every buy — the recurring purchases, rebalancing and redeploying proceeds — through the last day of the window, and shows in its row how many days remain until buying resumes. The window runs from the day the bot saw the sale fill. The day after it ends, the constituent is simply the most underweight name in the index and [rebalanced DCA](10-dollar-cost-averaging.md) buys it back.

Two limits. The bot counts only its own trades; a purchase of the same asset by another bot, by hand or on another account counts for the rule just the same. And a partial sale — a rebalance, or a Sell when the exchange holds less than the position — leaves the rest in place: if that name was bought within the window before the sale, part of the loss is washed by those purchases, and the bot cannot prevent it. Turn rebalancing off on a bot you harvest from, or accept it.

## Index changes

Companies join and leave the index. A constituent that has left is moved to the **Left the index** table and is no longer bought; selling it is your call, from its own row, because it is a disposal with tax consequences. See [Portfolio rebalancing](12-portfolio-rebalancing.md).
