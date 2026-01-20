![Frame 3 (4)](https://github.com/user-attachments/assets/ffd80181-014d-40ad-9669-b98e5c1f265e)

Auto-DCA for crypto. Automate your Dollar Cost Averaging strategy across multiple exchanges. As a service, [Deltabadger](https://deltabadger.com) helped users invest over $72 million into Bitcoin and other digital assets. Now it's free and open-source!

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
# Edit .env.docker as needed
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

Another app is using port 3737. Either stop that app, or change the port mapping. For example, to use port 4000:

**Single command:** Change `-p 3737:3000` to `-p 4000:3000`

**Docker Compose:** Set in `.env.docker`:
```bash
APP_PORT=4000
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
