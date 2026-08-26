# Welcome to Deltabadger

**Deltabadger** is your command center for growing and managing long-term portfolio of stocks and crypto.

It places recurring orders (DCA), rebalance portfolios, execute direct indexing, automatically widthraw crypto from exchanges your wallet, tracks what you hold, generate tax reports, and opens MCP/Rest API gate to the markets for your agents. No matter if you run it on your local computer, Umbrel, or an online server, it can be used by multiple users, and your [API keys stay there private and encrypted](40-secrets-and-encryption-keys.md).

## Invest. Track. Connect.

**Bots** can both build your portfolio by dollar-cost averaging, and manage it long-term with rebalancing. When you [creating your bot](08-creating-your-first-bot.md), you can use multiple smart triggers: price suddenly dropped, RSI below 30, price below moving average, and more. You can switch between DCA and portfolio rebalancing or do both, according to your current needs.

**Rebalanced DCA** is a unique Deltabadger way to DCA into multiple assets where rebalancing is with buying orders only which doesn't create tax events.

You can pick a single asset, a custom basket, or [follow one of popular indexes](17-index-bot.md).

[Withdrawal rules](19-withdrawal-rules.md) helps to limit your risk by regularly moving your assets from an exchange to an address in that exchange's address book once the balance passes a threshold, checked every 4 hours. See .

**Tracker** shows holdings, value and profit across the exchanges you connect with read-only keys, keeps the transaction ledger, and generates [tax reports](25-crypto-tax-report.md). See [Connecting exchanges](21-connecting-exchanges.md) and .

An **MCP server** and a **REST API** give Claude and your own scripts the same data. See [Connecting Claude](30-connecting-claude.md) and [REST API](32-rest-api.md).

Crypto trades on the exchanges under [Supported exchanges](27-supported-exchanges.md). Stocks and ETFs trade through Alpaca, and through Interactive Brokers with a Deltabadger.com connection. See [Stocks and ETFs](29-stocks-and-etfs.md).

## Market data

Trading and withdrawing need only your exchange API keys; prices, candles and everything the triggers use come from the exchange itself. A market data provider is needed on top for index bots, for tax reports, which price every transaction on its day, and for keeping the list of exchanges and assets up to date. Without one you have the list that shipped with the app.

A free CoinGecko API key, pasted under **Settings → Connect → Market Data**, covers all three. A Deltabadger.com connection adds market data with full price history, exchange proxies (your API traffic leaves from fixed IP addresses, so you can lock your keys to them on the exchange), the stock catalog without an Alpaca key, and Interactive Brokers. See [Market data](35-market-data.md).

## Ways to run it

| Method | Page |
|---|---|
| Docker, one command | [Quick start](02-quick-start.md) |
| Docker Compose, with an `.env.docker` file | [Docker Compose](03-docker-compose.md) |
| Desktop app on macOS or Linux, built from source | [Desktop app](04-desktop-app.md) |
| Umbrel app | [Umbrel](05-umbrel.md) |
