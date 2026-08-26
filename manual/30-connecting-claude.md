# Connecting Claude

Deltabadger runs an MCP server, so Claude and other AI clients that speak MCP can read your bots, balances and transactions, and — if you allow it — start bots, place orders and generate tax reports. What a client may do is up to you; see [Tools and permissions](31-tools-and-permissions.md).

## The MCP URL

Open **Settings → Connect**. The **MCP** widget shows your server URL under "Use this URL to connect Claude (and other AI models)". Click it to copy.

The URL is your instance address followed by `/mcp`, built from `APP_ROOT_URL` (see [Configuration](38-configuration.md)). Set that variable to an address the client can reach — `localhost` only works for a client on the same machine.

Add the URL wherever your client accepts a remote MCP server. There is no token to paste: the client registers itself with your instance and opens the authorization page below.

## Authorizing a client

The first time a client connects, your browser opens Deltabadger's **Authorize access** page. Sign in if you are not already. The page shows which client is asking ("*Name* wants to access your Deltabadger server"), warns that the application registered itself and Deltabadger has not verified it — only continue if you started this yourself — and lists what the access covers. Under "Choose what it may use over MCP:" there is a checkbox per group: **Read**, **Control**, **Trade**, **Tax & Reporting**. Only **Read** is ticked to begin with.

Untick what you do not want this client to have, then press **Connect**, or **Cancel** to refuse. The ticked groups become the client's grant, limited to the tools switched on at that moment; tools you switch on later are granted from the **Connected** list.

A client that also asked for REST API access shows a second set of checkboxes; see [REST API](32-rest-api.md).

The client receives a token that lasts an hour and renews it on its own. You do not authorize again unless you revoke the client.

## Connected clients

Below the tool toggles, **Connected** lists every client you have authorized with the date it connected. Each client has a toggle per group: **(some)** means it holds part of a group, **(REST API)** marks groups granted for the REST API. Switch a toggle on to grant the whole group (only tools you currently have on), or off to take it away. A tool switched off in the grid is off for every client at once, but stays in the client's grant until you remove it here.

**Revoke** disconnects a client and stops its tokens immediately. It has to authorize again to connect.

If Claude reports that a tool is disabled or not available to this client, see [Tools and permissions](31-tools-and-permissions.md).
