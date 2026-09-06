# Creating new bot

The `+` button on the /bots Dashboard creates new bot. You have two options:
* `Pick Assets` - pick one or more assets yourself (you can adjust it later)
* `Pick Index` - pick an index, and let it manage the allocation with [direct indexing](manual/13-direct-indexing.md)

## Pick Assets

Pick one of the exchanges. 

<p><img width="1250" height="1045" alt="Screenshot 2026-09-06 at 18 12 02" src="https://github.com/user-attachments/assets/c3414ab2-d332-4521-b93e-500c26fd74db" /></p>

Each exchange comes with instructions how to connect it. However, for Alpaca, [setup it first](manual/26-market-data.md#alpaca) in the settings.

Alternatively, you can start with assets and the app will show you on which exchanges they're available. 

Add one or more assets. You can edit your choice by clicking on the selected stack. You can also always change it later.

<p><img width="1251" height="843" alt="Screenshot 2026-09-06 at 18 14 53" src="https://github.com/user-attachments/assets/35870150-6bf8-4894-8c49-c8577902a3f1" /></p>


Finally, when you're ready pick the currency to spend.

<p><img width="1261" height="699" alt="Screenshot 2026-09-06 at 18 16 29" src="https://github.com/user-attachments/assets/7d9b61fd-f06a-4852-8ba7-7b550510d724" /></p>

Your bot has been created. [Finish setting](manual/10-dollar-cost-averaging.md) before you start.

## Pick Index

For [Direct Indexing](manual/13-direct-indexing.md), start with picking the index. 

> [!IMPORTANT]
> This is where either [Coingecko](manual/26-market-data.md#coingecko) or [Deltabadger subscription](https://deltabadger.com) setup is necessary. Coingecko provides over 500 cryptocurrency indexes, while official Deltabadger API offers also stock indexes based on Nasdaq-100 and S&P 500.

<p><img width="2056" height="1289" alt="Screenshot 2026-09-06 at 18 20 07" src="https://github.com/user-attachments/assets/b73c6e19-e88f-4471-b9b3-757b282367c9" /></p>

Then you pick exchange, and when you pick the currency to spend, you can also see which assets in the index are available for each spending currency. 

Your bot has been created. [Finish setting](manual/10-dollar-cost-averaging.md) before you start.
