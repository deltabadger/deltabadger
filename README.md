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

### Quick Start

1. **Download the Docker files:**

```bash
curl -O https://raw.githubusercontent.com/deltabadger/deltabadger/main/docker-compose.yml
curl -O https://raw.githubusercontent.com/deltabadger/deltabadger/main/.env.docker.example
```

2. **Create environment file:**

macOS/Linux:

```bash
cp .env.docker.example .env.docker
```

Windows (Command Prompt):

```cmd
copy .env.docker.example .env.docker
```

The example file works out of the box for local use.

3. **Start the app:**

```bash
docker compose up -d
```

First run downloads the pre-built image. Once complete, access the app at `http://localhost:3000`.

### Updating to a New Version

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

### Production Secrets

For online servers, generate proper secrets:

```bash
openssl rand -hex 64  # For SECRET_KEY_BASE and DEVISE_SECRET_KEY
openssl rand -hex 16  # For APP_ENCRYPTION_KEY
```

Edit `.env.docker` and replace the dev values.

### Building from Source

If you prefer to build the image locally instead of using the pre-built one:

```bash
docker compose build
docker compose up -d
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
