# Triggers

Triggers are the rules listed under the main sentence of a single-asset bot. The bot keeps its schedule from [Amount and interval](09-amount-and-interval.md), but a trigger decides whether an order is actually placed: only while the market meets a condition, only after it has met it once, or by switching the bot into selling when it does.

Triggers belong to single-asset bots. A [Portfolio bot](15-portfolio-bot.md) or an [Index bot](17-index-bot.md) has Smart Intervals, FeeCutter, Starting time and Rebalance, nothing else. Like every other setting, a trigger is locked while the bot runs: press **Stop**, change it, press **Start**.

## The mode

Each trigger sentence starts with a dropdown that sets what the trigger does:

| Mode | What happens |
|---|---|
| **Buy only** | Orders are placed only while the condition holds. When it stops holding, the status bar reads **Waiting for all conditions to be met** and the bot checks again, every minute for the price triggers and at each candle close for the moving average and RSI triggers. |
| **Start buying** | The bot waits for the condition once. After it has been met, orders follow the schedule and the condition is not checked again. |
| **Start selling** | The bot keeps buying and watches. The moment the condition is met it reverses into selling, see [Selling](12-selling.md). |

The price-drop trigger offers **Start buying** and **Start selling** only.

## Price

**Buy only when price is above / below / between …** with the price in the quote currency. **between** takes two prices and is available in **Buy only** mode. The line under the sentence shows the current price so you can see how far the market is from the condition.

## Price drop

**Start buying when price drops X% from its all-time high** or **24h high**. The default is 20% from the all-time high. The line under the sentence shows the current reference high.

## Moving average

**Buy only when the price is above / below the SMA 9 on daily candles.** Pick **SMA** or **EMA**, the period, and the candle timeframe: **hourly**, **4-hour**, **daily**, **3-day**, **weekly** or **monthly**. The line under the sentence shows the moving average on the last candle close.

## RSI

**Buy only when the RSI on daily candles is above / below 30.** The RSI uses a 14-candle period; the candle timeframes are the same as for the moving average. RSI is the only indicator offered.

## Don't spend more than

The last rule caps the total: **Don't spend more than 1000 USD**. It counts what the bot has bought since the rule was switched on, open orders included, and shows what is left, for example **(400 USD left)**. When the cap is reached the bot stops itself, the status bar reads **Paused** and the rule's line reads **The whole amount has been invested**; when [Email notifications](37-email-notifications.md) are set up, it also sends the "invested full amount" email. A bot whose cap is already reached cannot be started. Switch the rule off to reset the target.
