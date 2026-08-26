# Stocks and ETFs

Crypto works out of the box. Stock trading needs two things your container cannot invent: the list of tradeable symbols, and prices for them. Which broker you can use follows directly from who supplies those.

## Alpaca — available self-hosted

An Alpaca API key provides the tradeable symbol list, live prices and chart history all at once, so a container can run stocks entirely on its own. An admin connects a key once under Settings → Connect (paper or live) to build the catalog. Syncing the full US catalog takes a few minutes; after that, stocks appear in the bot wizard. Each user then connects their own Alpaca key when creating a bot — the admin key only keeps the catalog fresh, and it is never used to place anyone's orders.

## Interactive Brokers — hosted only

IBKR is a broker, not a market-data provider: it publishes no free price feed and no price history, and its market data is a paid, per-exchange subscription tied to each account. A self-hosted container therefore has no source for the prices and charts the app needs, so the IBKR option stays hidden unless the container is connected to deltabadger.com market data. This is a data limitation, not a licensing one. Maybe we'll find a way around.
