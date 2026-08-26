# Docker Compose

Alternative to the single command in [Quick start](02-quick-start.md), using Docker Compose:

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

## Docker commands reference

| Command | Description |
|---------|-------------|
| `docker compose up -d` | Start in background |
| `docker compose down` | Stop all containers |
| `docker compose pull` | Pull latest image |
| `docker compose logs -f` | View logs (Ctrl+C to exit) |
| `docker compose logs -f web` | View web server logs only |

## Starting fresh

If something goes wrong and you want to reset everything:

```bash
docker compose down
docker volume rm deltabadger_storage deltabadger_logs
```

> **Note:** This deletes all data. Volume names may vary — run `docker volume ls` to see all volumes.

## Production notes

Secrets are auto-generated on first run and stored in `/app/storage/.secrets` (inside the volume). These persist across container restarts and upgrades.

For production deployments:
- Use a reverse proxy (nginx, Traefik) for HTTPS
- Set `APP_ROOT_URL` and `HOME_PAGE_URL` to your domain in `.env.docker`

## Building from source

If you prefer to build the image locally instead of using the pre-built one:

```bash
docker compose -f docker-compose.build.yml up -d --build
```
