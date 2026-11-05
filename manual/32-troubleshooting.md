# Troubleshooting

## Docker: Container won't start

Check logs for errors:

```bash
docker compose logs deltabadger
```

Common fixes:
- Make sure Docker Desktop is running
- Reset everything (see [Starting fresh](03-docker-compose.md#starting-fresh))

## Docker: "Deltabadger didn't start"

When Deltabadger can't start on your data, it doesn't stop: it keeps the container up and shows a page at its address saying why and what to do, and the container reports unhealthy. Nothing trades meanwhile. The same line is in the logs. The lines you can meet after updating from 2.x, and what to do about each, are on [Updating](07-updating.md#if-it-doesnt-start).

## Docker: Port already in use

Another app is using port 3737. Either stop that app, or change the port mapping. For example, to use port 4000:

**Single command:** Change `-p 3737:3000` to `-p 4000:3000`

**Docker Compose:** Set in `.env`, next to `docker-compose.yml`:
```bash
APP_PORT=4000
```

## Desktop app

The window says what happened and what to do; see [What the window can show](04-desktop-app.md#what-the-window-can-show). If it keeps happening, contact support and send them the files in the data folder's `log/` folder (see [Where your data is](04-desktop-app.md#where-your-data-is)).
