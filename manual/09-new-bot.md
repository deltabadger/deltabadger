# Creating new bot

The `+` button on the /bots Dashboard creates new bot. You have two options:
* `Pick Assets` - pick one or more assets yourself (you can adjust it later)
* `Pick Index` - pick an index, and let it manage the allocation with [direct indexing](manual/13-direct-indexing.md)

## Pick Assets

Pick one of the exchanges. 

<img src="#">

Each exchange comes with instructions how to connect it. However, for Alpaca, [setup it first](manual/26-market-data.md#alpaca) in the settings.

Alternatively, you can start with assets and the app will show you on which exchanges they're available. 

Add one or more assets. You can edit your choice by clicking on the selected stack. You can also always change it later.s

<img src="#">

Finally, when you're ready pick the currency to spend.

<img src="#">

Your bot has been created. [Finish setting](manual/10-dollar-cost-averaging.md) before you start.

## Pick Index

For [Direct Indexing](manual/13-direct-indexing.md), start with picking the index. 

> [!IMPORTANT]
> This is where either [Coingecko](manual/26-market-data.md#coingecko) or [Deltabadger subscription](https://deltabadger.com) setup is necessary. Coingecko provides over 500 cryptocurrency indexes, while official Deltabadger API offers also stock indexes based on Nasdaq-100 and S&P 500.

<img src="#">

Then you pick exchange, and when you pick the currency to spend, you can also see which assets in the index are available for each spending currency. 

Your bot has been created. [Finish setting](manual/10-dollar-cost-averaging.md) before you start.