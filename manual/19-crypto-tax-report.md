# Crypto tax report

A CSV of the year's taxable disposals — proceeds, cost basis, gain or loss, holding period — worked out the way the selected country requires. It reads the same transactions the Tracker shows, so put the ledger in order first: link transfers, state missing values, fix the findings (see [Transactions and positions](23-transactions-and-positions.md)). It is a calculation aid, not tax advice.

## What you need

- At least one exchange connected on the Tracker (see [Connecting exchanges](21-connecting-exchanges.md)). The **Tax Report** button appears only then.
- Historical prices. Tax reports require market data (see [Market data](35-market-data.md)). With none configured, **Tax Report** opens a **CoinGecko API Key** form instead of the report. A free CoinGecko key covers only recent history; older transactions need a paid CoinGecko plan or a Deltabadger.com connection. Only the admin can add the key; other users are told to ask them.

## Generating a report

1. Open the Tracker and press **Tax Report**. The **Generate Report** modal opens.
2. Choose **Tax Report**. (**All** is the plain transactions export — see [Import and export](24-import-and-export.md).)
3. Pick **Country** and **Year**. The list offers the current year and the five before it; last year is preselected.
4. For Austria and Slovakia a checkbox appears: **Treat stablecoins as fiat (ask your accountant)**. In both countries a coin-to-coin swap is not a taxable event; with the box ticked a stablecoin (USDT, USDC, DAI and the like) counts as cash, so selling into one is a disposal.
5. Press **Generate**.

The report runs in the background. A bar shows **Generating tax report...** with a percentage, and the CSV downloads on its own when done — or the next time you open the Tracker. Each report downloads once; after that you are asked to generate a new one. The file is `deltabadger-tax-report-<country>-<year>.csv`. Your last choices are remembered.

Claude can produce the same report through the Tax & Reporting tools — see [Tools and permissions](31-tools-and-permissions.md).

## What is in the file

One row per disposal in the chosen year; earlier years are read only to establish cost basis. Headers are in the country's language. The base columns are date, acquisition date, asset, amount, proceeds, cost basis, gain/loss, currency, holding days, fee, exchange, transaction ID, and two flags: whether the cost basis was complete and whether any data was missing. France drops acquisition date, cost basis, holding days and the cost-basis flag; Sweden drops acquisition date and holding days; the Netherlands and Switzerland use a different layout (reference date, asset, amount, value, currency). Countries add their own columns (table below). Stocks and ETFs held at a broker are not here — see [Broker tax report](26-broker-tax-report.md).

After the disposals:

- **Income received** — staking rewards, lending interest, airdrops, mining and other income (dividends, interest, rebates), each valued on the day it arrived, with a total per type. Listed separately, not added to the gains.
- **Warnings** — prices that could not be found; unlinked deposits whose cost basis was assumed at market value on arrival (link each to its withdrawal to use the real basis); and a NOTE listing values you typed by hand in the Tracker.

> **Note:** WARNING rows directly under the header, in the country's language, flag an exchange that was never synced or whose last sync failed, and — when any price is missing — how many values are missing and that the report must not be filed as-is. Fix the data and generate again.

## Countries

| Country | Method | Currency | What the report adds |
|---|---|---|---|
| Germany | FIFO | EUR | A tax-exempt column after a holding period over one year |
| Austria | FIFO | EUR | Coin-to-coin swaps not taxed, basis carries over; an old-stock column for coins bought before 1 March 2021 and held over a year; stablecoin checkbox |
| France | PVCT | EUR | Total acquisition cost and portfolio value per disposal; coin-to-coin swaps not taxed |
| Italy | LIFO | EUR | A tax-rate column (26% before 2026, 33% from 2026); an exempt column for years up to 2024 when the year's gains stay under 2,000 |
| Spain | FIFO | EUR | — |
| Bulgaria | FIFO | EUR (BGN for 2025) | 10% expense deduction taken off each gain; summary with tax at 10% |
| Greece | FIFO | EUR | Summary with tax at 15% |
| Netherlands | Wealth snapshot | EUR | Holdings and their value on 1 January of the year, then total value, allowance, taxable wealth, deemed return and tax — allowance and rates are known for 2025 and 2026; other years use the 2026 figures — no disposal rows |
| Portugal | FIFO | EUR | A tax-exempt column after one year; a swap restarts the holding period but is not taxed |
| Switzerland | Wealth snapshot | CHF | Holdings and their value on 31 December, total only; a note that income is not included |
| Poland | FIFO | PLN | Coin-to-coin swaps not taxed |
| United Kingdom | Share pooling | GBP | **Matching Rule** per disposal |
| United States | FIFO | USD | **Term**: Short-term or Long-term |
| Sweden | Weighted average | SEK | Summary of gains and losses, 70% of losses deductible |
| Ireland | FIFO with the four-week rule | EUR | **Matching Rule**, **Period**; summary with the 1,270 annual exemption, CGT at 33% and the two payment dates |
| Denmark | FIFO | DKK | Per-asset summary; a loss is marked denied when the asset was bought again between acquisition and sale |
| Czech Republic | FIFO | CZK | A tax-exempt column with its reason — time test (held over three years) or value test (total proceeds for the year up to 100,000 — then every disposal is exempt); summary |
| Slovakia | FIFO | EUR | Term and tax-rate columns (19% short-term, 7% long-term); coin-to-coin swaps not taxed; stablecoin checkbox |
