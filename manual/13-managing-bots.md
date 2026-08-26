# Managing bots

Bots live on the **Bots** page and each bot has its own page. This is where you start, stop, rename, archive, delete and move bots between exchanges.

## The Bots page

Every bot is a tile with its asset or index, the exchange logo, its P/L, its name, the status bar and the **Start** / **Stop** button. Drag tiles to reorder them. Above the tiles sits the P/L of the whole account; click it to switch between percent and amount. A filter with **All**, **Active**, **Inactive** and **Archived** appears once you have bots in different states or an archived one. **New bot** opens the wizard, see [Creating your first bot](08-creating-your-first-bot.md).

With exactly one bot there is no list: the menu entry reads **Bot** and opens that bot directly.

## Start and Stop

**Start** is greyed out when the bot cannot run: the API key is not valid, an asset is not listed on the exchange (**Not all the assets are listed on …**), or a portfolio's allocations do not add up to 100%. While the bot runs its settings are locked. **Stop** cancels the schedule and the status reads **Paused**.

Starting a bot that has run before opens a choice. Inside the interval: **Continue original schedule** or **Skip it**, which orders immediately. Past it: **Do you want to invest … missed while the bot was paused …** with **Yes, buy and continue** or **Skip the missed part**.

## Statuses

| Status bar | Meaning |
|---|---|
| Countdown with a progress bar | Time to the next order |
| **Paused** / **Paused: …** | Stopped, with the reason when the bot stopped itself |
| **Setting orders…** | An order is being placed |
| **Waiting for all conditions to be met** | A [trigger](11-triggers.md) is not met |
| **Last order failed: … - Next try in …** | The order failed and will be retried |
| **The market is closed now.** with a countdown | A stock bot waiting for the market to open |
| **Archived** | See below |

## The ⋯ menu

**Change name** replaces the default name, which is the asset, basket or index the bot buys. **Archive** asks **Do you want to archive this bot?**; an archived bot leaves the list, trades nothing and keeps its history. Find it under the **Archived** filter, where **Reactivate** takes the place of **Start** and returns it to the stopped state. **Delete** asks **Do you want to delete this bot?** and removes the bot from Deltabadger for good; there is no undo. Archive instead if you may want the numbers later.

## Exchange and keys

The exchange chip at the top left of the bot page opens a menu. **Add new keys** replaces the current key, see [API keys](28-api-keys.md); when the key is not valid a **Connect** button sits next to the chip. Below the divider are the other exchanges that list the bot's assets, those with saved keys highlighted; picking one moves the bot there. The menu says when it cannot: **Stop the bot to move it to another exchange.**, **No other supported exchange lists …**, or **This exchange no longer operates — switch the bot to another one.**

## Switching bots

The bot name at the top of the page is a dropdown with **New bot +** and every other bot.
