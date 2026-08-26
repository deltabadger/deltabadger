# Selling

A single-asset bot can sell on a schedule as well as buy: to take profit in steps, to exit a position without timing the market, or to alternate between buying and selling on a [trigger](11-triggers.md). Portfolio and index bots do not sell this way; for them see [Index changes](18-index-changes.md).

## The ⇄ control

The ⇄ icon at the end of the main sentence rotates the bot through three states:

| State | Sentence | What a tick does |
|---|---|---|
| Buying | **Invest 100 USD / Week into BTC** | Buys BTC for 100 USD |
| Selling an amount | **Sell 0.01 BTC / Week to USD** | Sells 0.01 BTC |
| Selling for an amount | **Sell BTC for 100 USD / Week** | Sells as much BTC as is worth 100 USD at the order price |

A fourth press returns to buying. The control works only while the bot is stopped. If the bot has open orders you are asked to confirm, because reversing cancels them: **Reversing will cancel any open orders on this bot. Continue?**

The sell sentence starts with an empty amount. Fill it in; until you do the bot runs but sells nothing. The selling interval is separate from the buying one and starts out equal to it.

> **Note:** The bot sells from your exchange balance of the asset, not only from what it bought. Each tick sells the configured amount, capped by the free balance on the exchange.

## Options while selling

**Smart Intervals** splits the amount to sell into smaller units; it is not offered while selling for a quote amount. **FeeCutter** places limit orders the chosen percentage *above* the price instead of below. **Starting time** works as when buying. See [Order options](10-order-options.md).

## Triggers while selling

Every trigger keeps a separate setting for each direction, so switching the bot to selling does not carry the buy-side conditions over. The mode reads **Sell only**, **Start selling** or, as the flip, **Start buying**. The price-drop trigger mirrors: **Start selling when price rises X% from its 24h low / 7d low**. The RSI trigger defaults to above 70, the moving average to price above the SMA 9 on daily candles.

A buy-side trigger set to **Start selling** and a sell-side trigger set to **Start buying** turn the bot into a simple trading bot that alternates. A trigger flip keeps the selling state you chose with ⇄ (an amount or a quote amount).

## Don't sell more than

While selling, the spend cap becomes **Don't sell more than 0.5 BTC**. It counts what has been sold since the rule was switched on, open sell orders included. When the cap is reached the bot stops itself; the status bar reads **Paused** and the rule's line reads **The whole amount has been sold**. When [Email notifications](37-email-notifications.md) are set up it also sends the "sold full amount" email. Switch the rule off to reset the target.
