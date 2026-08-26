# Supported exchanges

Deltabadger trades crypto on thirteen exchanges and stocks through two brokers. Every venue below works for bots and for the [Tracker](21-connecting-exchanges.md) (Interactive Brokers connects through Settings, not the Tracker's own form); only some can run [withdrawal rules](19-withdrawal-rules.md). How to create the key each one needs is on the [API keys](28-api-keys.md) page.

## Crypto exchanges

| Exchange | Connect with | Withdrawal rules | Notes |
|---|---|---|---|
| Binance | API Key, Secret Key | Yes | Whitelist the IP before the trading permission can be enabled. |
| Binance.US | API Key, Secret Key | Yes | Same key setup as Binance. |
| Coinbase | API key ID, Secret | No | Needs an **Advanced API** key (Coinbase Developer Platform). |
| Kraken | API Key, Private Key | Yes | The Tracker needs **Query ledger entries** in addition to the trading permissions. |
| Bitget | API Key, Secret Key, API Passphrase | No | |
| KuCoin | API Key, API Secret, API Passphrase | No | |
| Bybit | API Key, API Secret | No | |
| MEXC | Access Key, Secret Key | Yes | |
| Gemini | API Key, API Secret | Yes | Key scope **Primary**. No IP whitelist step. |
| Bitvavo | API Key, API Secret | No | |
| Hyperliquid | Wallet Address, Agent Private Key | No | Decentralized; every order is public on-chain. Orders below 10 USDC are rejected. |
| BingX | API Key, Secret Key | No | |
| Bitrue | API Key, Secret Key | No | |

BitMart has shut down. It can no longer be picked or keyed; a bot still on it shows *This exchange no longer operates — switch the bot to another one*, and you can move it from the exchange chip (see [Managing bots](13-managing-bots.md)).

In the bot wizard, Binance, Binance.US, Coinbase and Kraken are listed first; the rest follow.

## Stock brokers

| Broker | Connect with | Notes |
|---|---|---|
| Alpaca | API Key, API Secret, Mode (**Paper** / **Live**) | Also lists crypto pairs. Stock orders wait for market hours. |
| Interactive Brokers | OAuth setup under **Settings → Connect → Interactive Brokers (beta)** | Requires a Deltabadger.com connection (see [Market data](35-market-data.md)). Whole shares only. |

Neither broker supports withdrawal rules. What each needs before stocks appear in the wizard is on the [Stocks and ETFs](29-stocks-and-etfs.md) page.

## Exchange proxies

Exchange calls can be routed through a per-exchange HTTP proxy, either set by a Deltabadger.com connect code or by the `PROXY_<EXCHANGE>` variables described in [Configuration](38-configuration.md); without one, Deltabadger calls the exchange directly.
