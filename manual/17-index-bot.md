# Index bot

An index bot invests on a schedule into a basket picked by market cap — the top coins overall or a category such as Layer 1 — and keeps the basket in step with the market.

## Requirements

Index bots need market data: a Deltabadger.com connection or a CoinGecko key, see [Market data](35-market-data.md). Without one the wizard opens on **Connect CoinGecko**, where an admin pastes a CoinGecko API key; other users are told to ask the admin. A bot without market data will not start: "A market data provider must be configured for Index bots."

## Creating one

Press **New bot** and choose the **Index** card ("Build your personal ETF"). The steps are **Connect CoinGecko** (skipped once market data is configured), **Pick index**, **Exchange**, **Connect exchange**, **Currency**, then a final settings page — see [Creating your first bot](08-creating-your-first-bot.md) for keys and currency. The exchange table's **Coins** column shows which of the index's coins each exchange lists — the first few tickers and a +N for the rest.

## Pick index

A searchable grid of tiles, each with a description and sample tickers:

- **Top Coins** — "Top cryptocurrencies by market cap. The S&P 500 of crypto."
- Category indices built from CoinGecko's coin categories, for example Layer 1, Layer 2, Meme, DeFi or AI. A category is offered only when at least 3 of its coins trade on a supported exchange; stablecoin categories are left out. The list is refreshed once a day.
- With a Deltabadger.com connection, indices provided by Deltabadger.com appear first; these can include stock indices — see [Stocks and ETFs](29-stocks-and-etfs.md).

The index cannot be changed once the bot has placed orders.

## Settings

The last step is the bot's settings. The main rule reads "Invest 100 USD / Week into Top 10" with two sliders:

- **Number of coins** — how many of the index's top coins to hold, 2 to 50, limited to what your exchange lists against your currency. New bots start at 10; an index with a fixed member list starts at its full size and you trim it down.
- **Allocation flattening** — 0% weights the coins purely by market cap, 100% gives every coin an equal share, anything between blends the two.

The **Index Preview** under the sliders shows the resulting composition on your exchange; click it to switch between pie and list. If the exchange lists fewer coins than you asked for, a note says so: "8 of 10 coins in the index are available on Kraken".

Below are **Smart Intervals**, **FeeCutter**, **Starting time** ([Order options](10-order-options.md)) and **Rebalance** ([Rebalancing](16-rebalancing.md)); index bots have no [triggers](11-triggers.md). Press **Start**: the bot is created, starts immediately and opens on its page, named after the index — "Top 10", "Layer 1 · 20".

## How the composition follows the market

Before every contribution and every rebalance check, the bot re-reads the index ranking and rebuilds its composition: the top N coins your exchange lists against your currency, weighted by market cap with your flattening applied. A coin you already hold is not dropped over a momentary price glitch; it leaves only when it falls out of the top N or the exchange stops listing it. A coin that has left stops receiving contributions and moves to the **Left the index** table; it is not sold automatically — see [Index changes](18-index-changes.md). Each contribution is split across the coins below their target weight, as on a [portfolio bot](15-portfolio-bot.md).
