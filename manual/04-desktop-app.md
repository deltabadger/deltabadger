# Desktop app

The desktop app runs Deltabadger on your own computer, with no Docker and no server. It is one window and an icon in the menu bar or system tray. Your bots run while the app is open.

## Installing

Download the file for your computer from the [releases page](https://github.com/deltabadger/deltabadger/releases), the same page the app's **Download page** links open:

| Computer | File |
|---|---|
| Mac with Apple silicon | `Deltabadger_darwin-aarch64.dmg` |
| Windows (64-bit) | `Deltabadger_windows-x86_64-setup.exe` |
| Linux (x86-64) | `Deltabadger_linux-x86_64.AppImage` |
| Debian or Ubuntu (x86-64) | `Deltabadger_linux-x86_64.deb` |

On a Mac, open the `.dmg` and drag Deltabadger into **Applications**, then open it from there. An app opened straight from the disk image can't update itself (see [Updating](07-updating.md#desktop-app)).

## The first start

The window shows **Starting Deltabadger…** and then the dashboard, already signed in. There is no setup page, no sign-up and no password: the app makes one account the first time it starts, in your computer's language and time zone (both can be changed under [Account settings](24-account-settings.md)).

The desktop app sends no email. Bot alerts appear as your computer's notifications, and on the bot's own page. See [Email notifications](28-email-notifications.md#desktop-app).

To use Deltabadger in a browser on the same computer, or to connect an AI assistant, open **Settings → Sign in a browser** and press **Show a code**. Enter the code on the browser's sign-in page within 10 minutes. It works once; **New code** replaces it. See [MCP server](22-mcp-server.md#desktop-app).

## Closing and quitting

Closing the window hides it. Deltabadger keeps running, and your bots keep trading, until you quit it: choose **Quit** from the tray icon's menu, or on a Mac **Quit Deltabadger** (⌘Q) from the app menu. **Show Deltabadger** in the tray menu brings the window back.

While the app is quit, nothing trades. After you quit, the engine finishes what it is doing and closes by itself, usually within seconds. Before touching the data folder, make sure no program called `deltabadger-engine` is still running (Activity Monitor on a Mac, Task Manager on Windows, `pgrep deltabadger-engine` on Linux).

## What the window can show

| Window says | What it means |
|---|---|
| **Starting Deltabadger…** | The app is starting. After a minute it adds that the first start after an update can take a few minutes while Deltabadger saves a copy of your data and updates it. Leave it open. |
| **Deltabadger is finishing its previous run.** | The last run is still closing. If this doesn't change, Deltabadger may be open in another program on this computer, for example in a terminal. Close it, or restart your computer. |
| **Deltabadger stopped unexpectedly.** | Your bots are paused until it runs again. Quit Deltabadger and open it again. |
| **Deltabadger didn't start.** followed by a reason | The reason says what to do, and most also say whether your data was changed. |
| **Deltabadger isn't responding.** | Wait a moment. If it doesn't change, quit Deltabadger and open it again. |

If one of these keeps coming back, contact support and send them the files in the log folder below; the window names it.

## Where your data is

| Computer | Data folder |
|---|---|
| Mac | `~/Library/Application Support/com.deltabadger.app` |
| Windows | `%APPDATA%\Deltabadger` |
| Linux | `~/.local/share/com.deltabadger.app` |

Inside it:

| Path | Contents |
|---|---|
| `db/production.sqlite3`, `db/production_queue.sqlite3` | Your bots, keys, history and scheduled work |
| `db/backups/` | Copies Deltabadger saved before updating your data (see [Data and backups](30-data-and-backups.md#copies-deltabadger-makes)) |
| `.secrets` | The keys your stored exchange keys are encrypted with. Without this file the data can't be read. |
| `log/deltabadger.log` | The log. Support asks for the files in `log/`. |

To back up, quit Deltabadger, wait until its engine has closed (see [Closing and quitting](#closing-and-quitting)), and copy the whole data folder. See [Data and backups](30-data-and-backups.md#desktop-app).

## Command line

The app's engine is a program called `deltabadger-engine`, next to the app's own program: on a Mac, `/Applications/Deltabadger.app/Contents/MacOS/deltabadger-engine`; with the `.deb`, `/usr/bin/deltabadger-engine`; on Windows, `deltabadger-engine.exe` in the folder Deltabadger was installed to. Support may ask you to run one of its commands (see [Configuration](29-configuration.md#commands)). Quit the app first and wait until its engine has closed, and point the command at the data folder, for example on a Mac:

```bash
D="$HOME/Library/Application Support/com.deltabadger.app"
STORAGE_DIR="$D/db" /Applications/Deltabadger.app/Contents/MacOS/deltabadger-engine check
```

`reset-password` is not needed on the desktop: the app has no password.

## Coming from 2.x

Deltabadger 2.x had no released desktop app. A 2.x app you built yourself kept its data in the folder above only if it was a release build; Deltabadger 3 takes that data over on its first start, after saving a copy (see [Updating from 2.x](07-updating.md#updating-from-2x)). A development build kept its data in the source checkout, which the app doesn't read.
