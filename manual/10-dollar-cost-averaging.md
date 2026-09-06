# Dollar-Cost Averaging

**Dollar-Cost Averaging (DCA)** is strategy when you invest a fixed amount of money on recuring schedule no matter what the current price is. When the price is lower, you get more shares or tokens, and when it's higher, you get less. Over time, you get a fair price below average price from the period. For everyday investors, this simple strategy is proven to outperform active trading.

> [!IMPORTANT]
> **Rebalanced DCA**
>
> When you pick more than one asset in a single bot, with every buy Deltabadger tries to **rebalance** your desired allocation, with buy orders only, so if the difference is bigger, the rebalancing will not be perfect, however you avoid taxable events. You can always activate full [Portfolio Rebalancing](manual/12-portfolio-rebalancing.md), but if you don't want to rebalance between assets at all, setup them separately one bot per asset.

## Schedule

For DCA, the only mandatory setting is your desired schedule.

<p><img width="434" height="80" alt="Screenshot 2026-09-06 at 18 23 03" src="https://github.com/user-attachments/assets/ab937fa8-2530-4235-a8e4-c4155d58ee9f" /></p>

The actual schedule can be affected by *Smart Intervals*.

## Smart Intervals

<p><img width="938" height="153" alt="Screenshot 2026-09-06 at 18 24 04" src="https://github.com/user-attachments/assets/c32bb161-78db-473a-98f4-5a951e7ff028" /></p>

All exchanges and brokers have **minimum order size**, so some schedules are impossible to execute directly. Sometimes, this minimum is defined in base currency, so the value in spending currency is changing.

Deltabadger solves it using **Smart Intervals**:

> [!EXAMPLE]
> You want to buy Bitcoin for 5 USD/day, but the minimum order size is 10 USD. Deltabadger spend 10 USD every 2 days, so on average you still achieve your desired ratio.

## FeeCutter

<p><img width="932" height="185" alt="Screenshot 2026-09-06 at 18 24 46" src="https://github.com/user-attachments/assets/34a1f71a-bb22-4894-9a12-3cbdc5ff787e" /></p>

Using limit orders can lower the price you pay for assets. For one, many exchanges offer lower trading fees when you use limit orders instead of instant market orders. However, this fee can be cancelled completely if you set your orders below the current price. 

> [!EXAMPLE]
> Binance fee is 0.1%. Set FeeCutter to 0.1% and cancel fee to zero.

> [!IMPORTANT]
> Limit orders can stay in the book open for a long time. You'll see them with `Cancel` button on the transaction list. The lower the order below the price, the longer you may wait.
