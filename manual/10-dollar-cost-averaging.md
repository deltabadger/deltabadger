# Dollar-Cost Averaging

**Dollar-Cost Averaging (DCA)** is a strategy where you invest a fixed amount of money on a recurring schedule, whatever the current price is. When the price is lower you get more shares or tokens; when it is higher you get fewer. Over time you end up with a fair price, below the average price over that period. For everyday investors, this simple strategy is proven to outperform active trading.

> [!IMPORTANT]
> **Rebalanced DCA**
>
> When you pick more than one asset in a single bot, every buy is used to **rebalance** towards your target allocation. Only buy orders are used, so a large drift is not corrected in one go — but you avoid taxable events. You can always switch on full [portfolio rebalancing](12-portfolio-rebalancing.md), and if you do not want to rebalance between assets at all, set them up separately, one bot per asset.

## Schedule

For DCA, the only mandatory setting is your desired schedule.

<p><img width="434" height="80" alt="Screenshot 2026-09-06 at 18 23 03" src="https://github.com/user-attachments/assets/ab937fa8-2530-4235-a8e4-c4155d58ee9f" /></p>

The actual schedule can be affected by *Smart Intervals*.

## Smart Intervals

<p><img width="938" height="153" alt="Screenshot 2026-09-06 at 18 24 04" src="https://github.com/user-attachments/assets/c32bb161-78db-473a-98f4-5a951e7ff028" /></p>

Every exchange and broker has a **minimum order size**, so some schedules cannot be executed directly. Sometimes that minimum is set in the base asset, so its value in your spending currency keeps moving.

Deltabadger solves it using **Smart Intervals**:

> [!TIP]
> You want to buy Bitcoin for 5 USD/day, but the minimum order size is 10 USD. Deltabadger spends 10 USD every 2 days, so on average you still get the rate you asked for.

## FeeCutter

<p><img width="932" height="185" alt="Screenshot 2026-09-06 at 18 24 46" src="https://github.com/user-attachments/assets/34a1f71a-bb22-4894-9a12-3cbdc5ff787e" /></p>

Using limit orders can lower the price you pay. For one thing, many exchanges charge a lower fee for a limit order than for an instant market order. Beyond that, the fee can be cancelled out completely by placing your orders slightly below the current price.

> [!TIP]
> The Binance fee is 0.1%. Set FeeCutter to 0.1% and the fee is cancelled out.

> [!IMPORTANT]
> A limit order can sit in the book for a long time. Open orders appear in the transaction list with a **Cancel** button. The further below the price you place the order, the longer you may wait.
