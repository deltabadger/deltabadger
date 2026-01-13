# Deltabadger
Auto-DCA for crypto. Automate your Dollar Cost Averaging strategy across multiple exchanges. As a service, [Deltabadger](https://deltabadger.com) helped users invest over $72 million into Bitcoin and other digital assets. Now it's free and open-source!

![bot](https://github.com/user-attachments/assets/efa94d9d-f663-4999-9a24-22bb909812b4)

![dashboard](https://github.com/user-attachments/assets/a388230f-b106-48b3-8fca-170dba16751d)

## About this release

**Release 1.0.0-beta** is the first attempt to make Deltabadger a standalone app. To make it possible, we had to get rid of legacy bots, so at the moment the app works only with Binance, Coinbase, and Kraken. To use other exchanges, check [release 0.9.0](https://github.com/deltabadger/deltabadger/releases/tag/v0.9.0).

1. Download release.
2. Run `./setup.sh` first.
3. Run `./start.sh` to use the app.

On Mac, if you close the app, it continues working in the background. You can find it on the topbar.

Scripts should work on Linux as well, but have not been tested.

For Windows, the best way at the moment is to use Docker.

Are you a developer? Jump on the [Telegram channel](https://t.me/deltabadgerchat) and help build the best DCA bot out there.

## Running with Docker

### Prerequisites

Install [Docker Desktop](https://www.docker.com/products/docker-desktop/) for your operating system.

After installation, make sure Docker is running (you should see the Docker icon in your system tray/menu bar).

To run the app locally, open a terminal in the /deltabadger folder, and:

### 1. Create environment file

macOS/Linux:

```bash
cp .env.docker.example .env.docker
```

Windows (Command Prompt):

```cmd
copy .env.docker.example .env.docker
```

The example file works out of the box for local use.

### 2. Start the app

```bash
docker compose up
```

First run takes a few minutes to download images and build. Once you see logs from `web` and `jobs`, the app is ready.

Access the app at `http://localhost:3000`.

> **Tip:** Add `-d` to run in the background: `docker compose up -d`

### Stopping the app

```bash
docker compose down
```

### Docker Commands Reference

**Start the app:**

```bash
docker compose up
```

Add `-d` to run in background. Add `--build` to rebuild after code changes.

**Stop the app:**

```bash
docker compose down
```

Stops and removes all containers and networks.

**Rebuild after changes:**

```bash
docker compose build
```

Add `--no-cache` to force a complete rebuild.

**View logs:**

```bash
docker compose logs -f
```

Add service name to filter: `docker compose logs -f web`

### Starting Fresh

If something goes wrong and you want to reset everything:

```bash
# Stop all containers
docker compose down

# Remove data volumes (deletes all data!)
docker volume rm deltabadger_storage deltabadger_logs
```

> **Note:** Volume names may vary. Run `docker volume ls` to see all volumes.

Nuclear option — removes all unused Docker data:

```bash
docker system prune -a --volumes
```

### Production Secrets

If you want to run it on an online server, generate proper secrets:

```bash
openssl rand -hex 64  # For SECRET_KEY_BASE and DEVISE_SECRET_KEY
openssl rand -hex 16  # For APP_ENCRYPTION_KEY
```

Edit `.env.docker` and replace the dev values.

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
yarn build --watch
```

### Running tests

```bash
bundle exec rspec
```

Auto-run tests on file changes:

```bash
bundle exec guard -c
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

Another app is using port 3000. Either stop that app, or change the port in `.env.docker`:

```bash
APP_PORT=3001
```

### macOS: fork() crash (development)

If you see this error when loading exchanges:

```
objc[86427]: +[__NSCFConstantString initialize] may have been in progress in another thread when fork() was called.
```

Add to your shell config:

```bash
export OBJC_DISABLE_INITIALIZE_FORK_SAFETY=YES
```

---

## License

[AGPL-3.0](LICENSE)
