# Portfolio overview

The top of the **Tracker** page reads your portfolio as one account: a chart, six figures and a card of what you hold. All of it comes from the transactions and balances the Tracker fetched (see [Connecting exchanges](21-connecting-exchanges.md)) and follows the exchange filter above it.

## Chart

The chart draws the portfolio's value day by day against the money put into it. **Value** plots both curves; **P/L** plots the difference. **30D**, **1Y** and **All** narrow the window — a range longer than your history is not offered. Hover a point to read its date, money and percentage in the headline. The history is swept from your first transaction forward when the page first opens, so a new account shows a spinner until it finishes.

## The six figures

| Figure | What it means |
|---|---|
| **Total invested** | Money that came in from outside: cash deposits at face value, coins deposited at their value on arrival, minus withdrawals at the cost the coins leave with. Buys, sells and swaps change nothing; a transfer between your own accounts cancels out. Cash spent on a trade that no deposit explains is counted as money in, so a venue that reports trades but not deposits still shows what they cost. |
| **Portfolio Value** | Every balance the exchanges report, at current prices. |
| **Fees paid** | Trading fees over the whole history. Already deducted from the two P/L figures. |
| **Realised P/L** | Gain or loss on everything sold, first-in-first-out. |
| **Unrealised P/L** | What you still hold, at current prices, minus what it cost. |
| **Total P/L** | Portfolio Value − Total invested. |

The hint under **Total invested** says how much of it arrived without a purchase behind it — rebates, airdrops, staking rewards, dust credits — counted at its value on arrival.

**Unrealised P/L** leaves out any holding the ledger cannot vouch for — one whose ledger and exchange disagree, or whose opening balance is missing — and shows **—** when no holding can be. The findings panel names the holding and offers to fix it — see [Transactions and positions](23-transactions-and-positions.md).

## Holdings

**Holdings · N** lists every asset the exchanges report, largest first, with a ring of the allocation. The header sums the list by kind — **Crypto 80% · Stock 10% · Stable 10%** — and the centre of the ring is the unrealised return on the positions it can vouch for: the coins held now against what they cost, unlike the chart, which measures the portfolio against every dollar ever put in.

Each row carries the logo, symbol and name, a type pill (**Crypto**, **Stock**, **ETF**, **Fund**, **Cash** or **Stable**), its share, its value and its unrealised percentage. Cash has no cost, so no return; a row without a reliable cost basis shows **—** (hover it for the reason). Rows beyond the sixth fold under **Show more**; slices under 2% fold into **Other** in the ring.

## Display currency

Every money figure on the page is shown in your display currency — USD, EUR, GBP, CHF or PLN — chosen under **Settings → Account** (see [Account settings](33-account-settings.md)).

## Hide balances

The **Hide balances** switch in the **Settings** menu removes every amount from the page: the six figures disappear, the chart becomes proportions with the invested line at 100 and loses its **Value / P/L** and range switches, and every money column drops from the tables — amount, value and fee for transactions; invested, average buy, price and the money beside the P/L for positions. Percentages, shares and holding periods stay.

## Ticker hover card

Wherever an asset appears as a ticker pill — in the holdings and transaction rows, on a bot page, in the bot wizard — hovering it shows a card with its logo, name, symbol and type.
