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


# API keys

An API key is how Deltabadger acts on your exchange account. Keys never leave your instance: they are stored encrypted in your own database (see [Secrets and encryption keys](40-secrets-and-encryption-keys.md)).

## Key types

| Type | Used by | Permissions |
|---|---|---|
| Trading key | Bots. The Tracker reads balances and history with it too. | Read + trade. No withdrawals. |
| Read-only key | Tracker only. | Read. A trading key already added for a bot does the same job, so you rarely need one. |
| Withdrawal key | Withdrawal rules. | Read + withdraw. Trading must be **off**. See [Withdrawal keys and addresses](20-withdrawal-keys-and-addresses.md). |

You can hold one key of each type per exchange. Adding a second one for the same exchange replaces the first.

## Where keys are managed

**Settings → Connect → API keys** lists every stored key under **Trading keys** and **Withdrawal keys** (read-only keys appear with the trading ones). Click the exchange name to reopen its **How to get API keys from …** guide; click the **×** to delete it. Deleting a trading key stops any running bot on that exchange first.

Keys are added where they are needed:

- Bot wizard, **Connect exchange** step (**Add API keys for …**).
- Tracker, **Connect exchange** (see [Connecting exchanges](21-connecting-exchanges.md)).
- Withdrawal rule wizard.
- On a bot's page, the exchange chip: **Connect** when the stored key does not work, or **Add new keys** to replace a working one while the bot is not running.

Each form shows the exchange's own field labels and, beside it, the numbered guide for that key type. Press **Connect**; it reads **Validating…** while the key is checked against the exchange.

## Permissions per exchange

Enable exactly what the guide lists. Binance, Binance.US and Coinbase reject a key that carries extra permissions; on the others, leave withdrawal and futures permissions off.

| Exchange | Trading key |
|---|---|
| Binance, Binance.US | **Enable Reading**, **Enable Spot & Margin Trading**. Whitelist the IP first — the trading option is locked until you do. |
| Coinbase | Portfolio **View** and **Trade** on an **Advanced API** key. |
| Kraken | Funds: **Query**. Orders & Trades: **Query open orders & trades**, **Query closed orders & trades**, **Create & modify orders**, **Cancel & close orders**. Data: **Query ledger entries** (used by the Tracker). |
| Bitget | **Read-write**, **Spot - Trade**, plus a **Passphrase**. |
| KuCoin | **General**, **Spot - Trade**, plus a **Passphrase**. |
| Bybit | **Read-Only**, **Spot - Trade**. |
| MEXC | **View Order Details**, **Trade**. |
| Gemini | Scope **Primary**; **Order Placement**, **Order Status**, **Account Balance**. |
| Bitvavo | **View**, **Trade**. |
| Hyperliquid | **Generate API agent** and copy the **Agent Private Key** (shown once). Never enter your main wallet key. |
| BingX | **Read-write**, **Spot - Trade**. |
| Bitrue | **Read Info**, **Trade**. |
| Alpaca | Key and secret from the Alpaca dashboard, and **Mode**: **Paper** (default) or **Live**. |
| Interactive Brokers | Not a key form — connect under **Settings → Connect → Interactive Brokers (beta)** first; the wizard then skips this step. See [Stocks and ETFs](29-stocks-and-etfs.md). |

For a read-only Tracker key on Binance or Binance.US, leave the default **Enable Reading** and nothing else. Other exchanges use the trading guide for the Tracker as well.

## IP whitelisting

Guides with a whitelist step tell you which address to enter (Gemini and Hyperliquid have none). With exchange proxies configured it is the proxy host; otherwise it is the public IP of the machine running Deltabadger, and the guide links to a page that shows it. On Binance the whitelist must be set before the trading permission can be ticked. A Binance read-only key works without a whitelist.

## Key status

- **Validating…** — the key is being checked.
- Valid — the wizard moves on; the Tracker starts syncing.
- **Incorrect API key permissions** — the exchange rejected the key, or its permissions do not match the guide. Create a new key.
- **Failed to validate API key permissions** — the check itself failed, usually because the exchange could not be reached. Try again.

A key that later stops working shows **Connect** on the bot's exchange chip, and the Tracker's exchange chip opens the re-add form. What a failed Tracker sync means is on [Connecting exchanges](21-connecting-exchanges.md).
