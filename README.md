# Deltabadger

[![Docker Build](https://github.com/deltabadger/deltabadger/actions/workflows/docker-publish.yml/badge.svg)](https://github.com/deltabadger/deltabadger/actions/workflows/docker-publish.yml)
[![Docker Image](https://img.shields.io/badge/ghcr.io-deltabadger%2Fdeltabadger-blue?logo=docker)](https://github.com/deltabadger/deltabadger/pkgs/container/deltabadger)
[![License](https://img.shields.io/github/license/deltabadger/deltabadger)](LICENSE)

[Deltabadger](https://deltabadger.com) is a one-stop-shop for investors in crypto and stocks:

No other tool offers this unique combination:

* **DCA bots** for crypto and stocks
* **Auto-withdrawals** to keep your assets safe
* **MCP Server** to connect your exchange accounts to Claude or Claw
* **Crypto Tax Reporting** tool with no transaction limits

For tax-reporting, and some more advanced features you'll need a free Coingecko account for market data.

### Quick Start

Install [Docker Desktop](https://www.docker.com/products/docker-desktop/) for your operating system, and make sure it's running, then run Deltabadger with a single command:

```bash
docker run -d --name deltabadger -p 3737:3000 -v deltabadger_data:/app/storage ghcr.io/deltabadger/deltabadger:latest standalone
```

That's it! Access the app at `http://localhost:3737`.


## Running with Tauri (macOS and Linux)

1. Download release.
2. Run `./setup.sh` first.
3. Run `./start.sh` to use the app.

On Mac, if you close the app, it continues working in the background. You can find it on the topbar.

Scripts should work on Linux as well, but have not been tested.

For Windows, the best way at the moment is to use Docker.

Are you a developer? Jump on the [Telegram channel](https://t.me/deltabadgerchat) and help build the best DCA bot out there.

## Running with Docker Compose

Alternative to the single command above, using Docker Compose:

1. **Download docker-compose.yml:**

```bash
curl -O https://raw.githubusercontent.com/deltabadger/deltabadger/main/docker-compose.yml
```

2. **Start the app:**

```bash
docker compose up -d
```

First run downloads the pre-built image. Secrets are auto-generated. Once complete, access the app at `http://localhost:3737`.

3. **Optional: Custom configuration**

Create `.env.docker` to override defaults (copy from `.env.docker.example` for reference):

```bash
curl -O https://raw.githubusercontent.com/deltabadger/deltabadger/main/.env.docker.example
cp .env.docker.example .env.docker
# Edit .env.docker as needed. Leave SECRET_KEY_BASE empty unless you have a
# reason not to — the container generates and persists a strong one on first run.
```

### Updating to a New Version

First, stop and remove the old container:

```bash
docker stop deltabadger && docker rm deltabadger
```

Then pull the latest image and run:

```bash
docker pull ghcr.io/deltabadger/deltabadger:latest
docker run -d --name deltabadger -p 3737:3000 -v deltabadger_data:/app/storage ghcr.io/deltabadger/deltabadger:latest standalone
```

Docker Compose:

```bash
docker compose pull
docker compose up -d
```

### Docker Commands Reference

| Command | Description |
|---------|-------------|
| `docker compose up -d` | Start in background |
| `docker compose down` | Stop all containers |
| `docker compose pull` | Pull latest image |
| `docker compose logs -f` | View logs (Ctrl+C to exit) |
| `docker compose logs -f web` | View web server logs only |

### Starting Fresh

If something goes wrong and you want to reset everything:

```bash
docker compose down
docker volume rm deltabadger_storage deltabadger_logs
```

> **Note:** This deletes all data. Volume names may vary — run `docker volume ls` to see all volumes.

### Production Notes

Secrets are auto-generated on first run and stored in `/app/storage/.secrets` (inside the volume). These persist across container restarts and upgrades.

For production deployments:
- Use a reverse proxy (nginx, Traefik) for HTTPS
- Set `APP_ROOT_URL` and `HOME_PAGE_URL` to your domain in `.env.docker`

### Moving to a new SECRET_KEY_BASE

`SECRET_KEY_BASE` is the encryption key for everything encrypted in this instance —
exchange API keys, your two-factor secret, withdrawal addresses, and market-data
configuration. You cannot simply change it on a running install: every encrypted field
is derived from it, so changing the value in place makes all of them unreadable and
locks you out. The steps below clear the stored credentials under the old key and let
you re-enter them under the new one.

**Who needs step 4.** Treat your credentials as compromised, and step 4 (revoke) as
not optional, if either of these is true:

- This install's `SECRET_KEY_BASE` was ever one of the placeholder values shipped in
  this repository, or any other value that has appeared somewhere public. Anyone
  holding a copy of your database can already read every credential in it.
- A person chose the value rather than generating it randomly. A short or
  human-chosen key is cheap to brute-force offline from a single encrypted database
  value or one captured session cookie, which yields the same result. A value not
  being on our placeholder list says nothing about how it was chosen, and neither
  check below can see how it was chosen either — add `-e PUBLISHED=yes` to the
  commands in steps 3 and 5 so they say so too.
- You have already changed `SECRET_KEY_BASE` and can no longer sign in. The stored
  credentials were written under the *previous* key, and the strong value you are
  running now tells you nothing about whether that previous one was published.

Only if your secret was randomly generated and has never left the machine is nothing
here known to be exposed. Then follow the same steps at your convenience and skip
step 4. If you are unsure, revoke.

The report and the reset make this call where they can, and say which way they went:
they treat the data as compromised if the secret currently in use is one published in
this repository, or if anything stored will not decrypt under it — the signature of
that third case. Neither check can see how a value was chosen, nor a key you have
already replaced and discarded. Those two are your call to make: add `-e PUBLISHED=yes`
to the report and reset commands to force the compromised wording.

Follow this order exactly. Back up only after stopping the app — a copy taken while
it is still writing is not consistent, and it is the only way back. Run the report
before revoking anything — it is what tells you which credentials exist; revoking
first means working from memory and missing the fee keys, SMTP password, or
market-data token. Revoke before the reset, and do not reissue anything until after
the restart — a replacement has nowhere to be stored until the app is running again,
and for Interactive Brokers, registering a new key early hands them a public key
whose private half the reset then deletes.

The commands below use the service name from `docker-compose.yml`. The Umbrel app
definition calls that service `web` — substitute it if you run from that file.

1. Stop the app.  `docker compose stop`
2. Back up your data, now that nothing is writing to it. The default deployment uses a
   named volume, not a host directory:
   ```bash
   docker compose cp deltabadger:/app/storage ./storage-backup
   ```
   If you changed `docker-compose.yml` to use a bind mount, copy that host directory
   instead. On a default install this copy includes `/app/storage/.secrets` — the key
   itself — so the backup decrypts itself. Keep it as carefully as the database, and
   do not put it anywhere shared. If your key comes from `.env.docker` or a compose
   file it is *not* in this copy, and step 6 overwrites it — save that value too, or
   this backup cannot be restored.
3. List what is stored:
   ```bash
   docker compose run --rm --no-deps deltabadger \
     rake deltabadger:encryption:report
   ```
   Copy your withdrawal addresses — they are not recoverable afterwards.
4. **REVOKE everything it listed, at its source: exchange API keys, fee keys, SMTP
   password, Alpaca key and secret, CoinGecko key, market-data token.** Revoke only —
   replacements have nowhere to live until step 7.
5. Clear what is stored:
   ```bash
   docker compose run --rm --no-deps -e CONFIRM=clear-credentials \
     deltabadger rake deltabadger:encryption:reset
   ```
6. Point this install at a new key. Blank the `SECRET_KEY_BASE` line in `.env.docker`,
   then delete the generated key, which is a separate file inside the volume:
   ```bash
   docker compose run --rm --no-deps deltabadger \
     rm -f /app/storage/.secrets
   ```
   On a default install `.env.docker` is already blank and `.secrets` is where your key
   actually is, so this deletion is what rotates anything at all. A new one is written
   only when that file is absent; an existing one is never overwritten.

   If your value comes from a compose file instead — the Umbrel app definition sets it
   from `APP_SEED` — put a freshly generated value in **both service blocks** rather
   than blanking it:
   ```bash
   openssl rand -hex 64
   ```
   That file sets `SECRET_KEY_BASE` twice, once under `web` and once under `jobs`, and
   each container's own environment beats the generated file. Use the same value in
   both: change one and the two halves of the app run on different keys, and neither
   can read what the other wrote.
7. Recreate the container so it picks up the edited value:
   ```bash
   docker compose up -d --force-recreate
   ```
   Starting the stopped container instead would bring it back with the old value
   still baked in: Compose reads `env_file` and `environment:` when a container is
   *created*, not when it starts, so editing the file does not reach a container
   that already exists. A strong per-install secret is generated for you.
8. Sign in, issue fresh credentials, add them, re-enable two-factor, re-issue your REST
   API token, reconnect your MCP clients, and restart your bots and rules — the reset
   cleared and stopped all of them.

Your REST API token and every connected MCP client are cleared by step 5, not by you in
step 4, and the reset prints how many it deleted. They are bearer tokens held in this
database and derived from nothing, so changing `SECRET_KEY_BASE` leaves them working and
still able to place orders — deleting them is what invalidates them. The OAuth client
registrations themselves survive, so a client can reconnect under the client ID it
already has, but it has to be authorized again.

Interactive Brokers is the slow one: its key pair is generated by this app and stored
here, so it can only be replaced after step 7 — run the connect wizard again, register
the new key in IBKR's portal, and wait for them to activate it. Registering a fresh
key any earlier would hand IBKR a public key whose private half step 5 then deletes.

### Building from Source

If you prefer to build the image locally instead of using the pre-built one:

```bash
docker compose -f docker-compose.build.yml up -d --build
```

---

## Development Setup

### Requirements

- Ruby 3.4.8
- Node.js 18.19.1

Use [asdf](https://asdf-vm.com) or your preferred version manager.

### 1. Install dependencies

```bash
bin/setup
```

### 2. Database

```bash
bundle exec rails db:prepare
```

### 3. Start the app

```bash
bin/dev
```

This starts the Rails server with Solid Queue (background jobs) running in-process via Puma.

Alternatively, run services separately:

Terminal 1 — Rails (with background jobs):

```bash
rails s
```

Terminal 2 — JavaScript bundler (optional, for live reloading):

```bash
npm run build:watch
```

### Running tests

```bash
bin/rails test
```

---

## Troubleshooting

### Docker: Container won't start

Check logs for errors:

```bash
docker compose logs web
```

Common fixes:
- Make sure Docker Desktop is running
- Try rebuilding: `docker compose build --no-cache`
- Reset everything (see "Starting Fresh" above)

### Docker: Port already in use

Another app is using port 3737. Either stop that app, or change the port mapping. For example, to use port 4000:

**Single command:** Change `-p 3737:3000` to `-p 4000:3000`

**Docker Compose:** Set in `.env.docker`:
```bash
APP_PORT=4000
```
---

## License

[AGPL-3.0](LICENSE)
