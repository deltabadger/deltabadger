# Market data

A market data provider gives Deltabadger prices and asset details it cannot get from your exchange alone. Open **Settings → Connect → Market Data** to choose one. Only the admin sees this widget.

## What needs it

Deltabadger ships with a bundled list of exchanges, assets and trading pairs, so DCA and portfolio bots, triggers, bot charts and withdrawal rules work with no provider at all — they read prices from the exchange. A provider is needed for:

- Index bots. The wizard refuses to continue: "A market data provider must be configured for Index bots."
- The Tracker's USD values. Balances still sync, but crypto holdings cannot be priced and the page warns "Balances synced, but USD pricing is unavailable right now". Stocks priced by the broker itself are unaffected.
- Tax reports, which need historical prices. Without a provider the **Tax Report** button opens a CoinGecko setup form instead.
- A display currency other than USD (see [Account settings](33-account-settings.md)).
- Keeping asset names, logos and market caps fresh, and picking up pairs an exchange listed after your install.

## None

Market data is off. The bundled asset list stays as it is.

## CoinGecko

A free CoinGecko account is enough for most uses. The form links to the steps: sign up at CoinGecko, open the API section, press **Get Your API Key Now**, choose **Create Free Account** below the pricing, fill the form, press **Create API key** and copy the key.

Paste it into **CoinGecko API Key** and press **Start**. The key is saved as is; nothing checks it here, so a wrong key only shows up later as failed prices. Press **Synchronize** afterwards to refresh every exchange's pair list — a "Syncing assets…" banner shows while it runs, and it takes a while because the sync pauses between exchanges to stay under CoinGecko's rate limit. (The CoinGecko form that opens from **Tax Report** does check the key and starts the sync itself.)

Once connected the widget reads "Connected to CoinGecko for live market data." and shows **Synchronize**, which refreshes every exchange's pair list (use it if some exchanges or assets are missing), and **Disconnect**, which removes the key and turns market data off.

The free plan covers about two years of price history. For a tax report on older transactions you need a paid CoinGecko plan or a Deltabadger.com connection.

## Deltabadger.com — paste connect code

Select this option, paste the **Connect code** from your Deltabadger.com Self-Hosted subscription and press **Connect**. The code configures market data and, where included, exchange proxies; the widget then reads "Connected to Deltabadger.com for market data and exchange proxies." (or "… Exchange proxies are unavailable."). An invalid or expired code is refused. **Disconnect** removes the connection, proxies included.

The same code can be entered on the setup page (see [First run](06-first-run.md)) or supplied as `CLAIM_TOKEN` (see [Configuration](38-configuration.md)).

## Locked by the environment

When `MARKET_DATA_URL` is set in the environment, the widget shows only that provider — named by `MARKET_DATA_PROVIDER_NAME`, or the URL itself — greyed out, and none of the choices above. Change the environment to change the provider.
