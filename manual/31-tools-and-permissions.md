# Tools and permissions

Every MCP tool has its own switch, so you decide whether a connected AI client can only look at your account or also act on it. Connecting a client is covered in [Connecting Claude](30-connecting-claude.md).

## Two layers of permission

A tool is usable only when both of these are true:

1. It is switched on in **Settings → Connect → MCP**. The grid groups the tools as **Read**, **Control**, **Trade** and **Tax & Reporting**. Click a tool to flip it, or click a group title to flip the whole group. By default **Read** and **Tax & Reporting** are on, **Control** and **Trade** are off. Switching a tool off takes effect immediately for every client, even mid-conversation.
2. The client was granted it. A client receives its grant when you authorize it, and the grant only contains tools that were switched on at that moment. Tools you enable later have to be granted to each client from the **Connected** list. A client only sees the tools that pass both checks.

These switches are separate from the REST API's; see [REST API](32-rest-api.md).

## Read

| Tool | What it does |
|---|---|
| **List bots** | View all DCA bots and their status, type, pair and exchange; filter by status |
| **Bot details** | View detailed bot info and performance: P/L, average price, invested, current value |
| **List exchanges** | View connected exchanges and their API key status |
| **Exchange balances** | Fetch live balances from one exchange |
| **Portfolio summary** | View global P/L and a per-bot breakdown |
| **List transactions** | View recent trades, optionally for one bot (up to 100) |
| **List open orders** | View currently open (unfilled) orders across exchanges; an exchange that cannot be asked is reported, not skipped |

## Control

| Tool | What it does |
|---|---|
| **Create bot** | Create and start a DCA bot: exchange, asset, quote currency, amount, interval (hour, day, week or month), optional label and start time |
| **Start bot** | Start a stopped or newly created bot |
| **Stop bot** | Stop a running bot |
| **Update bots** | Change amount or label on a stopped bot |
| **Start rule** | Start a stopped rule |
| **Stop rule** | Stop an active rule |
| **Update rules** | Change settings on a stopped rule: withdrawal percentage, maximum fee percentage, minimum amount, threshold type |

Rules are the [withdrawal rules](19-withdrawal-rules.md) you created in the app; a client can switch them on and off but cannot create one.

## Trade

| Tool | What it does |
|---|---|
| **Market buy** | Execute a market buy order on a connected exchange; amount in quote currency by default, or in the asset |
| **Market sell** | Execute a market sell order; amount in the asset by default, or in quote currency |
| **Limit buy** | Place a limit buy order at a specific price |
| **Limit sell** | Place a limit sell order at a specific price |
| **Cancel order** | Cancel an open order by its ID |

Trade tools work on crypto exchanges and, for stocks, on Alpaca (see [Stocks and ETFs](29-stocks-and-etfs.md)). They act on the exchange account directly and are not tied to a bot.

> **Note:** With **Trade** on and paper trading off, a client places real orders with real money. Keep the group off unless you want that.

## Paper Trading

Under the grid, **Enable paper trading for trade tools** makes the Trade tools simulate orders with real market prices. No real orders are placed, and every result is prefixed with `[DRY RUN]`. Use it to try a client's trading behaviour before letting it spend anything. The setting applies to MCP only; the REST API has no equivalent.

## Tax & Reporting

| Tool | What it does |
|---|---|
| **Tax jurisdictions** | View supported countries with their calculation method and currency |
| **Generate tax report** | Start background tax report generation for a country and year; can treat stablecoins as fiat |
| **Tax report status** | Check if a tax report is ready |
| **Download tax report** | Retrieve the generated tax report as CSV |
| **Export transactions CSV** | Export account transactions as CSV, with optional exchange and date filters |
| **Account transactions** | View Tracker transactions with exchange, date-range and type filters (up to 200) |

**Generate tax report** produces the [crypto tax report](25-crypto-tax-report.md) only; the [broker tax report](26-broker-tax-report.md) is not available over MCP. For most countries it needs market data (see [Market data](35-market-data.md)) and refuses otherwise. A finished report is also picked up by the Tracker the next time you open it.

The export tools hand a client your complete transaction history; grant **Tax & Reporting** only to clients that need it.
