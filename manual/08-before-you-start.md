# Before you start

## Market data

**Deltabadger** lets you search for assets and move bots between exchanges, but to do that it has to know the asset universe first. Out of the box it ships with a catalog of the top cryptocurrencies, so you will have no trouble finding Bitcoin, Zcash or anything else in the top 100.

> [!IMPORTANT]
> For the full catalog, connect a free [CoinGecko account](26-market-data.md#coingecko).

> [!IMPORTANT]
> To trade **stocks**, [connect Alpaca](21-supported-exchanges.md#stock-brokers) first and sync its asset catalog.

> [!NOTE]
> The **Interactive Brokers** API provides no asset catalog at all, and stock data APIs are all paid, so for now the only way to use it is with the [Deltabadger API](https://deltabadger.com/).

## Is your IP static?

You can run Deltabadger on any computer, but if you run it at home your IP address is most likely not static and will change from time to time. Some exchanges make you whitelist your IP for security reasons, and it is good practice anyway. Deltabadger shows the IP to whitelist during setup; after a couple of weeks it may change. Your options:

1. Do not whitelist an IP at all — though some exchanges require it.
2. Accept the hassle and update the whitelisted IP on the exchange whenever it changes.
3. Run Deltabadger on an online server, or on another network with a static IP.
4. Use the [Deltabadger Self-hosted plan](https://deltabadger.com/), which includes a proxy server.
