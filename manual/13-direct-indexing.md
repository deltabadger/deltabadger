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

Some tax systems disallow a loss if you buy the same asset back too soon: the United States (30 days), the United Kingdom (30 days, where the repurchase is matched against the sale instead of the pool) and Ireland (28 days). The rule is set once for the whole account, in [Account settings](24-account-settings.md) — what counts as a loss is a fact about you, not about one bot — and **every** bot then obeys it. After a sale at a loss the asset is left out of every buy: recurring purchases, rebalancing, redeploying proceeds, multi-asset legs, signal buys and the API's own buy endpoints. The money that buy would have spent goes to the next asset in line, so the schedule is not skipped. Selling is never blocked.

Each bot shows a **Wash sale protection** table listing the names it trades that are inside a window, with the days remaining, and the Account page lists every locked name across the whole account. The day after a window ends, the asset is simply the most underweight name again and [rebalanced DCA](10-dollar-cost-averaging.md) buys it back.

The first time you arm anything that can sell, Deltabadger asks you to choose — including "don't apply this rule". It asks once.

Four limits worth knowing:

- **Sales made outside Deltabadger are picked up by the account sync, not instantly.** A sale you make on the exchange's own website arms the window from the next sync, which runs daily and after every order a bot places. Buys that land in the gap are not protected.
- **What the sync cannot see, the rule cannot serve:** an exchange connected without a read key or with the permission revoked, Interactive Brokers, and swaps of one crypto for another, which the tracker does not treat as disposals.
- **Whether a bot's own sale was a loss is worked out from that bot's own purchase lots.** The account-wide walk that follows corrects it, within a day.
- **A partial sale** — a rebalance, or a Sell when the exchange holds less than the position — leaves the rest in place: if that name was bought within the window before the sale, part of the loss is washed by those earlier purchases, and no rule here can prevent it. Turn rebalancing off on a bot you harvest from, or accept it.

The window counts calendar days in the app's timezone, which can differ from your tax residence's by up to a day.

## Index changes

Companies join and leave the index. A constituent that has left is moved to the **Left the index** table and is no longer bought; selling it is your call, from its own row, because it is a disposal with tax consequences. See [Portfolio rebalancing](12-portfolio-rebalancing.md).
