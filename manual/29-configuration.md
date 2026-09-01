# Configuration

Deltabadger is configured through environment variables. A local install needs none of them: every value has a working default, and the secrets are generated on first start. Set them when you put the app on a domain, want email, or route exchange traffic through a proxy.

With Docker Compose, put them in `.env.docker` next to `docker-compose.yml` (copy `.env.docker.example`, see [Docker Compose](03-docker-compose.md)). With a single `docker run`, pass them with `-e NAME=value`. Compose reads `.env.docker` when it creates the container, so after editing it run `docker compose up -d --force-recreate`; a plain restart keeps the old values.

Three variables are an exception: Compose resolves `APP_PORT`, `APP_ROOT_URL` and `HOME_PAGE_URL` itself, from your shell or from a `.env` file next to `docker-compose.yml`. Values for those in `.env.docker` do not reach it.

## Ports and URLs

The container always listens on port `3000`; the host port is whatever you map to it.

| Variable | What it does |
|---|---|
| `APP_PORT` | Host port Compose maps to the container. Default `3737`. With `docker run`, change `-p 3737:3000` instead. |
| `APP_ROOT_URL` | The address people open, for example `https://dca.example.com`. Sets the host and protocol of links in emails, the origin allowed for live page updates, and the MCP and REST API URLs shown under **Settings → Connect**. Default `http://localhost:3737` with Compose; `http://localhost:3000` with `docker run`, so pass `-e APP_ROOT_URL=http://localhost:3737` there. |
| `ALLOWED_HOSTS` | Comma-separated hostnames the app answers to. `localhost` and `127.0.0.1` are always allowed. Unset, any host is accepted. |
| `FORCE_SSL` | `true` or `false`. Inferred from an `https://` `APP_ROOT_URL`, so set it only to override that: `true` turns on HSTS, secure cookies and `https` links for a non-https root URL; `false` serves an `https://` root URL over plain http. |
| `BEHIND_PROXY` | `true` or `false`. Whether a reverse proxy sits in front of the container. It decides which address the rate limits on sign-in, password reset and the other authentication pages count: the forwarded address when a proxy is declared, the connecting address otherwise. Inferred from an `https://` `APP_ROOT_URL` or `FORCE_SSL=true`; set `true` yourself for a proxy that talks plain http to the container. Left unset behind a proxy, everyone shares one budget and the sign-in page starts answering "Too many requests from this address. Wait a minute and try again." |

For HTTPS, put nginx, Traefik or similar in front of the container and set `APP_ROOT_URL` to the public address.

## Secrets

Leave these empty on a normal install. See [Secrets and encryption keys](40-secrets-and-encryption-keys.md) before changing any of them.

| Variable | What it does |
|---|---|
| `SECRET_KEY_BASE` | Session secret and, on older installs, the root of all stored-data encryption. Generated into `/app/storage/.secrets` on first start when empty. |
| `ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY`, `ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT` | Keys for stored data (API keys, two-factor secrets, addresses). Generated on a fresh install. Set both or neither; the app refuses to start with one. |
| `SECRETS_FILE` | Where the generated secrets are kept. Default `/app/storage/.secrets`. |

## Databases

Four SQLite files, all under `/app/storage` by default. See [Data and backups](39-data-and-backups.md).

| Variable | Default |
|---|---|
| `DATABASE_PATH` | `/app/storage/production.sqlite3` |
| `QUEUE_DATABASE_PATH` | `/app/storage/production_queue.sqlite3` |
| `CACHE_DATABASE_PATH` | `/app/storage/production_cache.sqlite3` |
| `CABLE_DATABASE_PATH` | `/app/storage/production_cable.sqlite3` |

## Email

Email can also be set up in the app; see [Email notifications](37-email-notifications.md). When `SMTP_ADDRESS` is set in the environment, **Settings → Account → Email Notifications** is locked to that server and shows it under the name from `SMTP_PROVIDER_NAME`. The example file sets `SMTP_ADDRESS=localhost`; blank it to set up email in the app instead.

| Variable | What it does |
|---|---|
| `SMTP_ADDRESS` | SMTP server hostname. Setting it turns on the environment SMTP option. |
| `SMTP_PORT` | SMTP port. |
| `SMTP_DOMAIN` | HELO domain sent to the server. |
| `SMTP_USER_NAME`, `SMTP_PASSWORD` | SMTP login. |
| `NOTIFICATIONS_SENDER` | From address. Falls back to the SMTP username, then `noreply@localhost`. |
| `SMTP_PROVIDER_NAME` | Label shown in Settings. Falls back to `SMTP_ADDRESS`. |

## Market data

See [Market data](35-market-data.md) for what each provider covers.

| Variable | What it does |
|---|---|
| `CLAIM_TOKEN` | A one-time Deltabadger.com connect code (`dbc_…`). Redeemed when the setup page is first opened: connects market data and exchange proxies and prefills the admin name and email. See [First run](06-first-run.md). |
| `MARKET_DATA_URL`, `MARKET_DATA_TOKEN` | A separately managed market-data service. While `MARKET_DATA_URL` is set, **Settings → Connect → Market Data** is locked to it. |
| `MARKET_DATA_PROVIDER_NAME` | Label shown in Settings for that service. Falls back to the URL. |
| `COINGECKO_API_KEY` | CoinGecko key used until one is saved in Settings. A key saved in Settings wins, even when it was cleared. |

## Exchange proxies

`PROXY_<EXCHANGE>` routes one exchange's API traffic through an HTTP proxy, for exchanges that require a fixed IP on the API key. The value is the proxy URL, for example `PROXY_BINANCE=http://proxy.example.com:8100`. Recognised names: `BINANCE`, `BINANCE_US`, `BINGX`, `BITGET`, `BITRUE`, `BITVAVO`, `BYBIT`, `COINBASE`, `GEMINI`, `HYPERLIQUID`, `KRAKEN`, `KUCOIN`, `MEXC`. A proxy supplied by a Deltabadger.com connection overrides the environment value for that exchange.

## Performance

| Variable | What it does |
|---|---|
| `WEB_CONCURRENCY` | Web server worker processes. Default `0`, a single process; values above `1` start that many workers. |
| `RAILS_MAX_THREADS` | Threads per process, and the base of the database connection pool. The image sets `1`; the Umbrel app sets `5`. |
| `MAX_DB_CONNECTIONS` | Worker pool for live page updates over websockets. Default `4`. |

## Other

| Variable | What it does |
|---|---|
| `AUTO_MIGRATE` | In `web` mode, `true` runs database migrations on start. `standalone` always migrates. |
| `RAILS_LOG_TO_STDOUT` | Set by the image. Logs go to the container output, so `docker compose logs -f` shows them. |
| `RAILS_SERVE_STATIC_FILES` | Set by the image. The app serves its own assets; keep it unless a web server in front serves `/public`. |

## Command modes

The word after the image name chooses what the container runs.

| Command | Runs |
|---|---|
| `standalone` | Web server and job worker in one process. Migrates the database on every start. Used by the quick-start command and `docker-compose.yml`. |
| `web` | Web server only. Migrates only when `AUTO_MIGRATE=true`. The image default when no command is given. |
| `jobs` | Job worker only: bots, rules, syncs, emails. Must share the storage volume with a `web` container. |

The Umbrel app runs `web` and `jobs` as two containers; see [Umbrel](05-umbrel.md).

## Maintenance commands

Run these against a stopped app, with the same volume and environment:

```bash
docker compose stop
docker compose run --rm --no-deps deltabadger <command>
```

| Command | Does |
|---|---|
| `migrate` | Creates or migrates the databases, and loads the asset list if it is empty. |
| `setup` | Prepares and seeds the databases. |
| `console` | Opens an interactive console on the app. |
| `rake <task>` | Runs a named task, for example `rake deltabadger:encryption:report`. See [Secrets and encryption keys](40-secrets-and-encryption-keys.md). |
| `shell` | Opens a shell inside the container. |

Anything else is run as given. On Umbrel, run them inside the `web` container instead; the command is on the [Umbrel](05-umbrel.md) page.

## Health endpoints

Both answer without signing in and are safe to poll.

| Endpoint | Response | Used by |
|---|---|---|
| `GET /health-check` | `{"health":"check"}`, without touching the database | `docker-compose.yml` |
| `GET /up` | `200` once the app has booted | The image's own health check, every 30 seconds |

If a container reports unhealthy, read its logs with `docker compose logs -f`. See [Troubleshooting](41-troubleshooting.md).
