# Umbrel

Deltabadger ships as an Umbrel app, so it runs on an Umbrel home server with no Docker commands to type.

## Installing

Open the Umbrel App Store, find **Deltabadger** in the Finance category and press **Install**. If it is not listed in your store, add this repository as a community app store: `https://github.com/deltabadger/deltabadger`.

The app listens on port `3737`, so it opens at your Umbrel's address on port `3737`, for example `http://umbrel.local:3737`. Umbrel does not put its own login in front of it: Deltabadger has its own accounts, and the first visit shows the setup page. Continue with [First run](06-first-run.md).

## Where data lives

Everything is kept in the app's data directory on your Umbrel, under `data/`:

| Folder | Contents |
|---|---|
| `data/storage` | The databases and `.secrets`, the file with this install's encryption keys |
| `data/logs` | Application logs |

The session secret is not in that folder. Umbrel derives it from the app's seed and passes it to both services as `SECRET_KEY_BASE`. A copy of `data/storage`, taken while the app is stopped, is a complete backup of a fresh install. If your install predates the `.secrets` file, its encryption keys are still derived from the Umbrel seed; run `rake deltabadger:encryption:derived_keys` first so they are stored in `.secrets` (see [Secrets and encryption keys](40-secrets-and-encryption-keys.md)). See also [Data and backups](39-data-and-backups.md).

## Two services

The Umbrel app runs the same image as two containers that share one database:

| Service | Container | Runs |
|---|---|---|
| `web` | `deltabadger_web_1` | The interface; runs database migrations on start |
| `jobs` | `deltabadger_jobs_1` | Bots, rules, exchange syncs, emails |

Bots run in `jobs`. If that container is stopped, nothing trades, even though the interface still opens. The container names follow Umbrel's `<app>_<service>_1` pattern; `docker ps` lists them. Read their logs with `docker logs deltabadger_web_1` and `docker logs deltabadger_jobs_1`.

The rest of this manual writes maintenance commands for the Docker Compose service, `docker compose run --rm --no-deps deltabadger …`. On Umbrel run them inside the running web container instead:

```bash
docker exec -it deltabadger_web_1 docker-entrypoint.sh rake deltabadger:encryption:report
```

Anything you set in the environment, such as your own encryption keys, has to be the same in both service blocks, or neither service can read what the other wrote.

## Updating

Update Deltabadger from the Umbrel App Store like any other app. Both services pull the `latest` image, and `web` migrates the database on start. The Docker steps on [Updating](07-updating.md) do not apply here.
