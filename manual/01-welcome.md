# Welcome to Deltabadger

**Deltabadger** is your command center for building and managing a long-term portfolio of stocks and crypto.

It places recurring orders (DCA), rebalances portfolios, runs direct indexing, withdraws crypto from exchanges to your own wallet, tracks what you hold, generates tax reports, and opens an MCP/REST API gateway to the markets for your agents. Run it on your own computer, on Umbrel or on an online server; it can serve several people at once, and your [API keys stay private and encrypted](31-secrets-and-encryption-keys.md) on your own instance.

<p><img width="512" height="322" alt="Screenshot" src="https://github.com/user-attachments/assets/65e1f71f-97b0-47bb-8dbc-982fecc6728f" /></p>

## Invest. Track. Connect.

**Bots** can both build your portfolio by dollar-cost averaging and manage it long-term with rebalancing. When you [create a bot](09-new-bot.md), you can use several smart triggers: price suddenly dropped, RSI below 30, price below the moving average, and more. You can switch between DCA and portfolio rebalancing, or do both, as your needs change.

**Rebalanced DCA** is Deltabadger's own way of dollar-cost averaging into several assets at once: it rebalances with buy orders only, so it never creates a taxable event.

You can pick a single asset, a custom basket, or [follow one of the popular indexes](13-direct-indexing.md).

[Withdrawal rules](16-withdrawal-rules.md) limit your risk by moving assets from an exchange to an address in that exchange's own address book once the balance passes a threshold. Balances are checked every 4 hours.

**Tracker** shows holdings, value and profit across the exchanges you connect with read-only keys, keeps the transaction ledger, and generates [tax reports](19-crypto-tax-report.md). See [Portfolio Tracker](17-portfolio-tracker.md).

An **MCP server** and a **REST API** give Claude and your own scripts the same data. See [MCP server](22-mcp-server.md) and [REST API](23-rest-api.md).

Crypto trades on the exchanges under [Supported exchanges](21-supported-exchanges.md). Stocks and ETFs trade through Alpaca, and through Interactive Brokers with a Deltabadger.com connection — see [Stock brokers](21-supported-exchanges.md#stock-brokers).

## Market data

Trading and withdrawing need only your exchange API keys; prices, candles and everything the triggers use come from the exchange itself. A market data provider is needed on top for index bots, for tax reports, which price every transaction on its day, and for keeping the list of exchanges and assets up to date. Without one you have the list that shipped with the app.

A free CoinGecko API key, pasted under **Settings → Connect → Market Data**, covers all three. A Deltabadger.com connection adds market data with full price history, exchange proxies (your API traffic leaves from fixed IP addresses, so you can lock your keys to them on the exchange), the stock catalog without an Alpaca key, and Interactive Brokers. See [Market data](26-market-data.md).

## Ways to run it

| Method | Page |
|---|---|
| Docker, one command | [Quick start](02-quick-start.md) |
| Docker Compose, with an `.env.docker` file | [Docker Compose](03-docker-compose.md) |
| Desktop app on macOS or Linux, built from source | [Desktop app](04-desktop-app.md) |
| Umbrel app | [Umbrel](05-umbrel.md) |
