# MCP server

Deltabadger runs an MCP server, so Claude and other AI clients that speak MCP can read your bots, balances and transactions, and — if you allow it — create and manage bots, place orders, keep the Tracker up to date and generate tax reports. What a client may do is up to you; see [Tools and permissions](#tools-and-permissions).

## The MCP URL

Open **Settings → MCP & REST API**. The **MCP** widget shows your server URL under "Use this URL to connect Claude (and other AI models)". Click it to copy.

The URL is your instance address followed by `/mcp`, built from `APP_ROOT_URL` (see [Configuration](29-configuration.md)). Set that variable to an address the client can reach — `localhost` only works for a client on the same machine.

Add the URL wherever your client accepts a remote MCP server. There is no token to paste: the client registers itself with your instance and opens the authorization page below.

## Authorizing a client

The first time a client connects, your browser opens Deltabadger's **Authorize access** page. Sign in if you are not already. The page shows which client is asking ("*Name* wants to access your Deltabadger server"), warns that the application registered itself and Deltabadger has not verified it — only continue if you started this yourself — and lists what the access covers. Under "Choose what it may use over MCP:" there is a checkbox per group: **Read**, **Control**, **Trade**, **Tax & Reporting**. Only **Read** is ticked to begin with.

Untick what you do not want this client to have, then press **Connect**, or **Cancel** to refuse. The ticked groups become the client's grant, limited to the tools switched on at that moment; tools you switch on later are granted from the **Connected clients** list.

A client that also asked for REST API access shows a second set of checkboxes, "Choose what it may use over the REST API:"; see [REST API](23-rest-api.md).

The client receives a token that lasts an hour and renews it on its own. You do not authorize again unless you revoke the client.

## Connected clients

**Connected clients**, at the bottom of the page, is a card per client you have authorized, with the date it connected. Each card is a small grid: a row per group, a switch under **MCP** and one under **REST API**. A surface the client never asked for shows **–** instead of a switch.

A switch is on when the client holds everything you currently have enabled in that group, and half-set when it holds only part of it. Switch it on to grant the whole group (only the tools you currently have on), or off to take the group away. A tool switched off in the matrix above is off for every client at once, but stays in the client's grant until you remove it here.

**Revoke** disconnects a client and stops its tokens immediately. It has to authorize again to connect.

If Claude reports that a tool is disabled or not available to this client, see [Tools and permissions](#tools-and-permissions).

## Tools and permissions

**Tool permissions** is one table for both surfaces: a row per tool, grouped as **Read**, **Control**, **Trade** and **Tax & Reporting**, with a switch column for **MCP** and one for **REST API**. Click a switch to flip that tool on that surface; click the switch on a group row to flip the whole group. The two columns are independent — switching **List bots** on for MCP does not switch it on for REST.

On MCP, **Read** and **Tax & Reporting** start on, **Control** and **Trade** start off. Every REST switch starts off. Switching a tool off takes effect immediately for every client, even mid-conversation.

### Two layers of permission

A tool is usable only when both of these are true:

1. It is switched on in the **MCP** column of **Tool permissions**.
2. The client was granted it. A client receives its grant when you authorize it, and the grant only contains tools that were switched on at that moment. Tools you enable later have to be granted to each client from the **Connected clients** list. A client only sees the tools that pass both checks.

### Read

| Tool | What it does |
|---|---|
| **List bots** | View all bots and their status, type, pair and exchange; filter by status |
| **Bot details** | View detailed bot info and performance: P/L, average price, invested, current value; for an index or basket bot also its holdings that left the composition and any pending redeploy offer |
| **List exchanges** | View connected exchanges and their API key status |
| **Exchange balances** | Fetch live balances from one exchange |
| **Portfolio summary** | View global P/L and a per-bot breakdown |
| **List transactions** | View recent trades, optionally for one bot (up to 100) |
| **List open orders** | View currently open (unfilled) orders across exchanges; an exchange that cannot be asked is reported, not skipped |
| **List rules** | View your [withdrawal rules](16-withdrawal-rules.md) with status, exchange, asset, destination and thresholds; destinations are masked |
| **List indices** | View the indices an [index bot](13-direct-indexing.md) can track and the exchanges each is available on |

### Control

| Tool | What it does |
|---|---|
| **Create bot** | Create and start a [DCA](10-dollar-cost-averaging.md) bot: one asset, or a basket of 2–20 with weights (or market-cap weighting), plus exchange, quote currency, amount, interval (hour, day, week or month), optional label and start time |
| **Create index bot** | Create and start an [index bot](13-direct-indexing.md): an index from **List indices**, how many assets to hold, and how far to flatten market-cap weights towards equal |
| **Start bot** | Start a stopped or newly created bot |
| **Stop bot** | Stop a running bot |
| **Update bots** | Change settings on a stopped bot: amount and label on any bot, the index knobs on an index bot, the weights on a basket bot. Membership is not editable |
| **Archive bot** | Stop a bot and take it off the dashboard, keeping its history (see [Managing bots](15-managing-bots.md)) |
| **Reactivate bot** | Bring an archived bot back, stopped |
| **Delete bot** | Delete a bot, running or not; its schedule is cancelled and its history is kept but hidden |
| **Create rule** | Create a [withdrawal rule](16-withdrawal-rules.md), stopped. The address must already be on the exchange's withdrawal allow-list |
| **Delete rule** | Delete a stopped withdrawal rule |
| **Start rule** | Start a stopped rule |
| **Stop rule** | Stop an active rule |
| **Update rules** | Change settings on a stopped rule: withdrawal percentage, maximum fee percentage, minimum amount, threshold type |
| **Sync tracker** | Refresh the [Portfolio tracker](17-portfolio-tracker.md) — pull new account transactions and balances from every connected exchange, in the background |
| **Mark transfer** | Mark a withdrawal and a deposit as one transfer between your own accounts, so it is not a disposal, or undo that. Give either side; the match is found within 14 days |
| **State a price** | State the price of one account transaction the venue did not value — a deposit, a gift, an airdrop. Tiles, chart and tax reports use it |

### Trade

| Tool | What it does |
|---|---|
| **Market buy** | Execute a market buy order on a connected exchange; amount in quote currency by default, or in the asset |
| **Market sell** | Execute a market sell order; amount in the asset by default, or in quote currency |
| **Limit buy** | Place a limit buy order at a specific price |
| **Limit sell** | Place a limit sell order at a specific price |
| **Cancel order** | Cancel an open order by its ID |
| **Sell exited holding** | Sell, at market, a holding an index or basket bot no longer includes (see [Portfolio rebalancing](12-portfolio-rebalancing.md)) |
| **Answer redeploy offer** | Accept or decline an index or basket bot's "Redeploy?" offer — accept buys the idle sale proceeds back into the composition at target weights, decline takes them off the table for good |

Trade tools work on crypto exchanges and, for stocks, on Alpaca (see [Stock brokers](21-supported-exchanges.md#stock-brokers)). The first five act on the exchange account directly and are not tied to a bot.

> **Note:** With **Trade** on and paper trading off, a client places real orders with real money. **Sell exited holding** is an irreversible market sale and a taxable disposal. Keep the group off unless you want that.

### Paper Trading

The **MCP** widget has a switch, **Enable paper trading for trade tools**, that makes the Trade tools simulate orders with real market prices. No real orders are placed, and every result is prefixed with `[DRY RUN]`. Use it to try a client's trading behaviour before letting it spend anything. The setting applies to MCP only; the REST API places real orders.

### Tax & Reporting

| Tool | What it does |
|---|---|
| **Tax jurisdictions** | View supported countries with their calculation method and currency |
| **Generate tax report** | Start background tax report generation for a country and year; can treat stablecoins as fiat, and can replace a report that already exists |
| **Tax report status** | Check if a tax report is ready |
| **Download tax report** | Retrieve the generated tax report as CSV |
| **Export transactions CSV** | Export account transactions as CSV, with optional exchange and date filters |
| **Account transactions** | View Tracker transactions with exchange, date-range and type filters (up to 200) |

**Generate tax report** produces the [crypto tax report](19-crypto-tax-report.md) only; the [broker tax report](20-broker-tax-report.md) is not available over MCP. For most countries it needs market data (see [Market data](26-market-data.md)) and refuses otherwise. A finished report is also picked up by the Tracker the next time you open it.

The export tools hand a client your complete transaction history; grant **Tax & Reporting** only to clients that need it.
