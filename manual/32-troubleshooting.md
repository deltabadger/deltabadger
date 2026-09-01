# Troubleshooting

## Docker: Container won't start

Check logs for errors:

```bash
docker compose logs web
```

Common fixes:
- Make sure Docker Desktop is running
- Try rebuilding: `docker compose build --no-cache`
- Reset everything (see [Starting fresh](03-docker-compose.md#starting-fresh))

## Docker: Port already in use

Another app is using port 3737. Either stop that app, or change the port mapping. For example, to use port 4000:

**Single command:** Change `-p 3737:3000` to `-p 4000:3000`

**Docker Compose:** Set in `.env.docker`:
```bash
APP_PORT=4000
```
