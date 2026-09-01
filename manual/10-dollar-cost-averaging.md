# Dollar-Cost Averaging

**Dollar-Cost Averaging (DCA)** is strategy when you invest a fixed amount of money on recuring schedule no matter what the current price is. When the price is lower, you get more shares or tokens, and when it's higher, you get less. Over time, you get a fair price below average price from the period. For everyday investors, this simple strategy is proven to outperform active trading.

> [!IMPORTANT]
> **Rebalanced DCA**
>
> When you pick more than one asset in a single bot, with every buy Deltabadger tries to **rebalance** your desired allocation, with buy orders only, so if the difference is bigger, the rebalancing will not be perfect, however you avoid taxable events. You can always activate full [Portfolio Rebalancing](manual/12-portfolio-rebalancing.md), but if you don't want to rebalance between assets at all, setup them separately one bot per asset.

## Schedule

For DCA, the only setting is your desired schedule.

<img src="#">

The schedule you set is affected by:

## Smart Intervals

All exchanges and brokers have **minimum order size**, so some schedules are impossible to execute directly. Sometimes, this minimum is defined in base currency, so the value in spending currency is changing.

Deltabadger solves it using **Smart Intervals**:

> [!EXAMPLE]
> You want to buy Bitcoin for 5 USD/day, but the minimum order size is 10 USD. Deltabadger spend 10 USD every 2 days, so on average you still achieve your desired ratio.

