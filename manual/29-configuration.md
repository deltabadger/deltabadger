# Configuration

Deltabadger is configured through environment variables. A local install needs none of them: every value has a working default, and the secrets are generated on first start. Set them when you put the app on a domain, want email, or route exchange traffic through a proxy.

With Docker Compose, put them in `.env.docker` next to `docker-compose.yml` (copy `.env.docker.example`, see [Docker Compose](03-docker-compose.md)). With a single `docker run`, pass them with `-e NAME=value`. Compose reads `.env.docker` when it creates the container, so after editing it run `docker compose up -d --force-recreate`; a plain restart keeps the old values.

Two variables are an exception: Compose resolves `APP_PORT` and `APP_ROOT_URL` itself, from your shell or from a `.env` file next to `docker-compose.yml`. Values for those in `.env.docker` do not reach it.

## Ports and URLs

The container always listens on port `3000`; the host port is whatever you map to it.

| Variable | What it does |
|---|---|
| `APP_PORT` | Host port Compose maps to the container. Default `3737`. With `docker run`, change `-p 3737:3000` instead. |
| `APP_ROOT_URL` | The address people open, for example `https://dca.example.com`. Sets the host and protocol of links in emails, the origin allowed for live page updates, and the MCP and REST API URLs shown under **Settings → MCP & REST API**. Default `http://localhost:3737` with Compose; `http://localhost:3000` with `docker run`, so pass `-e APP_ROOT_URL=http://localhost:3737` there. |
| `ALLOWED_HOSTS` | Comma-separated hostnames the app answers to. `localhost` and `127.0.0.1` are always allowed. Unset, any host is accepted. |
| `FORCE_SSL` | `true` or `false`. Inferred from an `https://` `APP_ROOT_URL`, so set it only to override that: `true` turns on HSTS, secure cookies and `https` links for a non-https root URL; `false` serves an `https://` root URL over plain http. |
| `BEHIND_PROXY` | `true` or `false`. Whether a reverse proxy sits in front of the container. It decides which address the rate limits on sign-in, password reset and the other authentication pages count: the forwarded address when a proxy is declared, the connecting address otherwise. Inferred from an `https://` `APP_ROOT_URL` or `FORCE_SSL=true`; set `true` yourself for a proxy that talks plain http to the container. Left unset behind a proxy, everyone shares one budget and the sign-in page starts answering "Too many requests from this address. Wait a minute and try again." |

For HTTPS, put nginx, Traefik or similar in front of the container and set `APP_ROOT_URL` to the public address.

## Secrets

Leave these empty on a normal install. See [Secrets and encryption keys](31-secrets-and-encryption-keys.md) before changing any of them.

| Variable | What it does |
|---|---|
| `SECRET_KEY_BASE` | Session secret and, on older installs, the root of all stored-data encryption. Generated into `/app/storage/.secrets` on first start when empty. |
| `ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY`, `ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT` | Keys for stored data (API keys, two-factor secrets, addresses). Generated on a fresh install. Set both or neither; the app refuses to start with one. |
| `SECRETS_FILE` | Where the generated secrets are kept. Default `/app/storage/.secrets`. |

## Databases

Two SQLite files, both under `/app/storage` by default. See [Data and backups](30-data-and-backups.md).

| Variable | Default |
|---|---|
| `DATABASE_PATH` | `/app/storage/production.sqlite3` |
| `QUEUE_DATABASE_PATH` | `/app/storage/production_queue.sqlite3` |

`DATABASE_URL` and the other `*_DATABASE_URL` variables are refused: Deltabadger doesn't start while one is set.

## Email

Email can also be set up in the app; see [Email notifications](28-email-notifications.md). When `SMTP_ADDRESS` is set in the environment, **Settings → Account → Email Notifications** is locked to that server and shows it under the name from `SMTP_PROVIDER_NAME`. The example file sets `SMTP_ADDRESS=localhost`; blank it to set up email in the app instead.

| Variable | What it does |
|---|---|
| `SMTP_ADDRESS` | SMTP server hostname. Setting it turns on the environment SMTP option. |
| `SMTP_PORT` | SMTP port. |
| `SMTP_DOMAIN` | HELO domain sent to the server. |
| `SMTP_USER_NAME`, `SMTP_PASSWORD` | SMTP login. With either set, the server must offer STARTTLS or delivery fails; without them, STARTTLS is used when offered. |
| `NOTIFICATIONS_SENDER` | From address. Falls back to the SMTP username, then `noreply@localhost`. |
| `SMTP_PROVIDER_NAME` | Label shown in Settings. Falls back to `SMTP_ADDRESS`. |

## Market data

See [Market data](26-market-data.md) for what each provider covers.

| Variable | What it does |
|---|---|
| `CLAIM_TOKEN` | A one-time Deltabadger.com connect code (`dbc_…`). Redeemed when the setup page is first opened: connects market data and exchange proxies and prefills the admin name and email. See [First run](06-first-run.md). |
| `MARKET_DATA_URL`, `MARKET_DATA_TOKEN` | A separately managed market-data service. While `MARKET_DATA_URL` is set, **Settings → Connect → Market Data** is locked to it. |
| `MARKET_DATA_PROVIDER_NAME` | Label shown in Settings for that service. Falls back to the URL. |
| `COINGECKO_API_KEY` | CoinGecko key used until one is saved in Settings. A key saved in Settings wins, even when it was cleared. |

## Exchange proxies

`PROXY_<EXCHANGE>` routes one exchange's API traffic through an HTTP proxy, for exchanges that require a fixed IP on the API key. The value is the proxy URL, for example `PROXY_BINANCE=http://proxy.example.com:8100`. Recognised names: `BINANCE`, `BINANCE_US`, `BINGX`, `BITGET`, `BITRUE`, `BITVAVO`, `BYBIT`, `COINBASE`, `GEMINI`, `HYPERLIQUID`, `KRAKEN`, `KUCOIN`, `MEXC`. A proxy supplied by a Deltabadger.com connection overrides the environment value for that exchange.

## Other

| Variable | What it does |
|---|---|
| `DELTABADGER_UPDATE_CHECK` | `false` stops the twice-daily check for a newer release. See [Updating](07-updating.md). |
| `DELTABADGER_PLATFORM` | How this copy was installed — `docker`, `umbrel` or `desktop` — which decides what **Settings → Account** offers when an update exists. Set for you by the Compose file, the Umbrel app and the desktop app. Set it yourself only if you start the container some other way and want the Compose instructions: `docker` if you use Compose, otherwise leave it unset and the manual is linked instead. |
| `SETUP_TOKEN` | Locks the first-run setup page. While set, the page only opens at `/setup?token=<value>`, and everyone else sees **Setup is locked**. Meant for platforms that start the server before its owner arrives and then send the owner that link. Leave it unset otherwise — a value that is set but empty locks setup for everyone. See [First run](06-first-run.md). |

## Commands

The word after the image name, or the compose file's `command:`, chooses what the container runs. `web`, `standalone` and `serve` all run the app: the web interface, your bots and all background work, in one process. It updates the data on start. With no command, the image runs `web`.

Other commands are run against a stopped app, with the same volume and environment:

```bash
docker compose stop
docker compose run --rm --no-deps deltabadger <command>
```

| Command | Does |
|---|---|
| `check` | Checks that this version can start on the data, and what it would update, without changing anything. |
| `handback` | Returns a 2.x install to 2.70.21; see [Going back to 2.x](07-updating.md#going-back-to-2x). |
| `reset-password <email>` | Sets an account's password; see [Account settings](24-account-settings.md#forgot-your-password). It also runs while the app does, with `docker compose exec`. |
| `sync ledger` or `sync balances` | One Alpaca history or balance sync by hand. |
| `resolve-placement`, `resolve-withdrawal`, `reconcile-order`, `repair-binance-history` | For support: each settles one specific problem, and support tells you the exact command. |
| `shell` | Opens a shell inside the container. |
| `migrate`, `setup` | Do nothing in Deltabadger 3, which updates its data itself when it starts. They are kept so older compose files keep working. |
| `jobs` | Does nothing: 2.x ran background work in a separate `jobs` container, which is no longer needed. It waits until it is stopped. |
| `console`, `rake`, `export` | Refused: "This command isn't available in this version of Deltabadger." |

Anything else is run as given inside the container. On Umbrel, see [Umbrel](05-umbrel.md).

When a container is stopped, the app finishes what it is doing first. `docker-compose.yml` gives it 60 seconds (`stop_grace_period`); plain `docker stop` gives 10 unless you pass `-t`.

## Health endpoints

Both answer without signing in and are safe to poll.

| Endpoint | Response |
|---|---|
| `GET /health-check` | `{"health":"check"}` while the app runs. The image's own health check polls it every 30 seconds, and so does `docker-compose.yml`. |
| `GET /up` | `200` while the app runs. |

While Deltabadger is refused or still waiting to start (see [If it doesn't start](07-updating.md#if-it-doesnt-start)), both answer `503`, so the container reports unhealthy, and every other address shows the page that says why. Read the logs with `docker compose logs -f`. See [Troubleshooting](32-troubleshooting.md).
