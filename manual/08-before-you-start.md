# Before you start

## Market Data

**Deltabadger** allows you to search assets, and move bots across different exchanges, but for that is has to know the asset universe first. Out of box, it provides a catalog of top cryptocurrencies, so you'll have not problem to find Bitcoin, Zcash, or other assets from the top 100. 

> [!IMPORTANT]
> To access full catalog, you need to connect a free [Coingecko account](manual/26-market-data.md#coingecko).

> [!IMPORTANT]
> To trade **stocks**, you need to [connect Alpaca](manual/26-market-data.md#alpaca) first, and sync its asset catalog.

> [!INFO]
> **Interactive Brokers** API doesn't provide asset catalog at all, and stocks APIs are all paid, so at the momement the only way to use it is with [Deltabadger API](https://deltabadger.com/).

## Is your IP static?

You can run Deltabadger on any computer, but if you run it from home, most likely your IP is not static and it changes from time to time. Some exchange forces you tu whitelist your IP for security reasons, and it's generally advised to do so. Deltabadger shows your IP to whitelist during setup, after two weeks it may change. Your options:

1. Don't whitelist IP at all (not all exchanges allow it).
2. Accept the hussle and update the whitelisted IP on the exchange when it changes.
3. Run Deltabadger on your online server or other network with static IP.
4. Use [Deltabadger Self-hosted plan](https://deltabadger.com/) with proxy server included.