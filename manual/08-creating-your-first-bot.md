# Creating your first bot

A DCA bot buys a fixed amount of one asset at a fixed interval on an exchange account you connect with an API key. The wizard only asks for the exchange, the key, the asset and the currency; the amount, interval and options are set on the bot's page afterwards.

## New bot

On the **Bots** page press **New bot** and pick the **DCA / Rebalance** card. The other card, **Index**, is covered in [Index bot](17-index-bot.md).

## Pick exchange

The wizard opens on **Pick exchange**: a grid of supported exchanges. Click one.

The sentence above the list reads "You can also start with assets". Click **assets** to pick the asset first and see only exchanges that trade it. Either way the sentence fills in as you go — "Buy BTC on Kraken spending USD" — and clicking a filled part takes you back to that step.

## Connect exchange

Enter the key and secret of an API key created on the exchange (some exchanges also ask for a passphrase). The numbered guide beside the form shows where to create the key and which permissions it needs; where the exchange supports IP whitelisting it also says which IP to enter (the machine running Deltabadger, or the exchange proxy when one is configured, see [Configuration](38-configuration.md)). Press **Connect**; the key is validated before the wizard moves on. If a valid key for this exchange already exists, this step is skipped. See [API keys](28-api-keys.md).

## Pick asset

Search and click the asset to buy. One asset makes a DCA bot. You can keep adding assets, up to 20 — two or more turn the bot into a portfolio bot, see [Portfolio bot](15-portfolio-bot.md). Click the chosen assets in the sentence to open the basket and remove one with **×**. If no supported exchange lists every chosen asset, the wizard says so.

Stocks and ETFs appear in the search when a stock catalog is active; picking one sends you to a broker instead of a crypto exchange; when more than one broker lists it, the **Pick exchange** step shows the brokers to choose from. See [Stocks and ETFs](29-stocks-and-etfs.md).

## Currency

Pick the currency to spend. Only currencies the exchange pairs with your asset are listed. This is the last step: the bot is created and its page opens.

## Start

The new bot is not running yet. Its page shows the default schedule, **Invest 100 USD / Week into BTC**, with the [amount and interval](09-amount-and-interval.md), [order options](10-order-options.md) and [triggers](11-triggers.md) below it.

Press **Start**. The first order is placed right away and the next ones follow the interval, unless a starting time or a trigger holds them back. **Start** stays disabled while the API key is not valid, the exchange no longer lists the pair (a hint "Not all the assets are listed on …" appears beside it), or a setting is invalid — for example a starting date that has already passed. Stopping, restarting and renaming are described in [Managing bots](13-managing-bots.md).
