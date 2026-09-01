# Data and backups

Everything Deltabadger stores lives in one directory inside the container, `/app/storage`, on the volume mounted there. A copy of that directory, taken while the app is stopped, is a complete backup.

## What is in the storage directory

| File | Contents |
|---|---|
| `production.sqlite3` | Accounts, bots, rules, exchange keys, transactions, settings |
| `production_queue.sqlite3` | Scheduled work: bot runs, syncs, emails |
| `production_cache.sqlite3` | Cache: market data, chart series, computed metrics |
| `production_cable.sqlite3` | Live page updates |
| `.secrets` | This install's generated keys; see [Secrets and encryption keys](40-secrets-and-encryption-keys.md) |

Each database may have `-wal` and `-shm` companions next to it. Copy the directory whole, never single files. The paths can be changed on [Configuration](38-configuration.md).

Logs are not in the volume. They go to the container output: `docker compose logs -f`, or `docker logs deltabadger` for the quick-start container. On Umbrel, read them with `docker logs deltabadger_web_1` and `docker logs deltabadger_jobs_1`; see [Umbrel](05-umbrel.md).

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

If the keys came from the environment, the restored install has to run with the same values, or nothing stored under them can be read. Restore into the same version of Deltabadger the backup was taken from, or a newer one: `standalone` mode migrates the databases on start.

To wipe an install instead, see [Starting fresh](03-docker-compose.md#starting-fresh).

## Desktop app

The desktop app built with `setup.sh` keeps its databases in the `storage/` folder of the source checkout. Quit the app before copying it.
