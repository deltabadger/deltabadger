# Updating

## Docker

First, stop and remove the old container:

```bash
docker stop deltabadger && docker rm deltabadger
```

Then pull the latest image and run:

```bash
docker pull ghcr.io/deltabadger/deltabadger:latest
docker run -d --name deltabadger -p 3737:3000 -v deltabadger_data:/app/storage ghcr.io/deltabadger/deltabadger:latest standalone
```

## Docker Compose

```bash
docker compose pull
docker compose up -d
```
