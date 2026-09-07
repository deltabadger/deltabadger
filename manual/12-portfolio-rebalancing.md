# Portfolio rebalancing

Any multi-asset bot can become a portfolio rebalancer. By default it uses **rebalanced DCA**, where every purchase is aimed at restoring your target allocation, but nothing is ever sold. That is a great approach for tax purposes while you are still accumulating.

Full rebalancing can be switched on as well, and it works independently from DCA. When it is on, it **places orders whether DCA is on or off**. While DCA is running, full rebalancing is triggered very rarely — only when buying alone cannot restore the target allocation.

<p><img width="440" height="47" alt="Screenshot" src="https://github.com/user-attachments/assets/5e35d40a-cb11-4822-be89-f97d36654a55" /></p>

Deltabadger rebalances whenever an asset drifts further from its target than the threshold you set. It checks every four hours, and it refreshes the bot's targets first, so a bot that has been stopped for a while never rebalances towards an outdated allocation.

> [!NOTE]
> Traditional portfolio rebalancing was usually executed on a fixed schedule, but beta tests clearly show the superiority of threshold-based rebalancing. Every transaction costs fees and slippage, so it pays to act only when the price movement is big enough. On top of that, time-based rebalancing can miss the right moment completely.

## Index bot

Rebalancing works on [index bots](13-direct-indexing.md) as well, but assets that drop out of the index are not sold automatically. You sell them yourself, one at a time, from the **Left the index** table.

<img width="661" height="182" alt="Screenshot 2026-09-07 at 12 18 44" src="https://github.com/user-attachments/assets/ce6e40f9-9e0d-4a38-827e-a5af1ac1b822" />

> [!IMPORTANT]
> Assets tend to leave an index and come back later. Selling them automatically would liquidate a large position the moment any other asset drifted out of its band, while an asset that left at 0.1% of the portfolio would never trip the band and would sit there forever. Each sale is also a taxable event, so the timing is yours to pick. For now, this is a manual step.
