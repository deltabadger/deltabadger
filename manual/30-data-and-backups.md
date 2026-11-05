# Data and backups

Everything Deltabadger stores lives in one directory inside the container, `/app/storage`, on the volume mounted there. A copy of that directory, taken while the app is stopped, is a complete backup. The desktop app keeps the same files in its own data folder; see [Desktop app](#desktop-app).

## What is in the storage directory

| File | Contents |
|---|---|
| `production.sqlite3` | Accounts, bots, rules, exchange keys, transactions, settings |
| `production_queue.sqlite3` | Scheduled work |
| `.secrets` | This install's generated keys; see [Secrets and encryption keys](31-secrets-and-encryption-keys.md) |
| `backups/` | Copies Deltabadger made itself; see [Copies Deltabadger makes](#copies-deltabadger-makes) |

Each database may have `-wal` and `-shm` companions next to it. Copy the directory whole, never single files. The paths can be changed on [Configuration](29-configuration.md).

An install that started on 2.x also has `production_cache.sqlite3` and `production_cable.sqlite3`. Deltabadger 3 no longer uses them, and leaves them where they are.

Logs are not in the volume. They go to the container output: `docker compose logs -f`, or `docker logs deltabadger` for the quick-start container. On Umbrel, see [Umbrel](05-umbrel.md).

The quick-start command names the volume `deltabadger_data`; `docker-compose.yml` defines one called `storage`, prefixed with the project directory name. `docker volume ls` shows the real names. Files inside are owned by user id `1000`; the container corrects ownership on every start unless you run it with `user:` set, in which case make the copied files owned by `1000:1000` yourself.

## Backing up

1. Stop the app, so nothing is writing to the databases:
   ```bash
   docker compose stop
   ```
2. Copy the directory out of the stopped container:
   ```bash
   docker compose cp deltabadger:/app/storage ./storage-backup
   ```
   For the quick-start container use `docker stop deltabadger` and `docker cp deltabadger:/app/storage ./storage-backup`. If you changed the compose file to use a bind mount, copy that host directory instead.
3. Start the app again with `docker compose start` (or `docker start deltabadger`).

> **Note:** On a default install the copy contains `.secrets`, the key that decrypts every API key in it, so the backup is as sensitive as your exchange accounts. Keep it as carefully as the database and never put it anywhere shared. If `SECRET_KEY_BASE` or the encryption keys come from `.env.docker`, a compose file or Umbrel instead, they are not in the copy — save those values with it, or the backup cannot be read.

## Restoring

1. Stop the existing container with `docker compose stop`, or create a new one without starting it: `docker compose create` (with plain Docker, `docker create` with the same options as the quick-start command).
2. Copy the backup in, replacing whatever is there:
   ```bash
   docker compose cp ./storage-backup/. deltabadger:/app/storage
   ```
   (`docker cp ./storage-backup/. deltabadger:/app/storage` for the quick-start container.)
3. Start it: `docker compose up -d` or `docker start deltabadger`.

If the keys came from the environment, the restored install has to run with the same values, or nothing stored under them can be read. Restore into the same version of Deltabadger the backup was taken from, or a newer one: Deltabadger updates the data on start, and refuses data from a newer version than itself.

A restored backup knows nothing of the orders Deltabadger placed after it was taken. Before you start any bot again, check each exchange's open orders and order history, so a bot doesn't buy again what was already bought.

To wipe an install instead, see [Starting fresh](03-docker-compose.md#starting-fresh).

## Copies Deltabadger makes

Before Deltabadger changes your data in a way an older version can't read, it saves a copy of the two database files in `backups/`, next to the database:

| Folder | Made |
|---|---|
| `pre-3.0-<date and time>` | On the first start of Deltabadger 3 on a 2.x install, before taking it over. Also on any later start of 3 on an install that 2.x ran last, for example after a [handback](07-updating.md#going-back-to-2x). |
| `before-update-<date and time>` | Desktop app only: on the first start after an update that changes the data. |

A start that finds nothing changed since the newest copy of its kind makes no new one, and the log says so. Each copy holds `production.sqlite3`, `production_queue.sqlite3` and a `digest` file with their checksums. Not `.secrets`: the copy is read with the same keys as the install.

Deltabadger never deletes these copies, so they add up: each is as big as your database files. Once 3 runs your bots and you are content with it, you may delete any of them; the oldest `pre-3.0-` copy is the one to keep longest, as it is your 2.x data as it was. A folder ending in `.partial` is a copy that was interrupted; the next start deletes it and makes it again.

### Restoring a copy Deltabadger made

A copy is for data that was damaged, not a way to undo an update: restoring it loses every order Deltabadger placed after the copy was made. To go back to 2.x and keep those orders, use [`handback`](07-updating.md#going-back-to-2x) instead.

With Docker Compose, to restore a `pre-3.0-` copy and run 2.x again:

1. Stop Deltabadger and list the copies:
   ```bash
   docker compose stop
   docker compose run --rm --no-deps deltabadger ls /app/storage/backups
   ```
2. Copy the two files over the install's, and delete the databases' `-wal` and `-shm` files. With `pre-3.0-20261105T120000.000000Z` standing for the copy you chose:
   ```bash
   docker compose run --rm --no-deps deltabadger sh -c 'cd /app/storage &&
     cp backups/pre-3.0-20261105T120000.000000Z/production.sqlite3 backups/pre-3.0-20261105T120000.000000Z/production_queue.sqlite3 . &&
     rm -f production.sqlite3-wal production.sqlite3-shm production_queue.sqlite3-wal production_queue.sqlite3-shm'
   ```
3. Change the `image:` line in `docker-compose.yml` to `ghcr.io/deltabadger/deltabadger:2.70.21` and start it: `docker compose up -d`.
4. Before you start any bot again, check each exchange's open orders and order history.

The desktop app's `before-update-` copies are restored the same way, by hand, into the data folder's `db/`, with the app quit; see [Going back to an earlier version](07-updating.md#going-back-to-an-earlier-version).

## Desktop app

The desktop app keeps everything in its data folder: `~/Library/Application Support/com.deltabadger.app` on a Mac, `%APPDATA%\Deltabadger` on Windows, `~/.local/share/com.deltabadger.app` on Linux. The databases are in its `db/` folder, the copies in `db/backups/`, the keys in `.secrets` and the log in `log/`. See [Desktop app](04-desktop-app.md#where-your-data-is).

To back up, quit Deltabadger (from the tray menu, or ⌘Q on a Mac; closing the window isn't enough) and copy the whole data folder. To restore, quit it, put the folder back, and open the app. The copy contains `.secrets`, so keep it as carefully as the note above says.
