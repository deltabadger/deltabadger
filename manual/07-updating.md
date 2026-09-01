# Updating

**Settings → Account** tells you when a newer version is out, and how to install it the way you are running Deltabadger. It is checked twice a day in the background, so the notice appears within half a day of a release; no page ever waits on the check.

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

Your data lives in the `deltabadger_data` volume, not in the container, so removing the old container keeps everything. The new one migrates the database on start.

## Docker Compose

```bash
docker compose pull
docker compose up -d
```

## Umbrel

Update Deltabadger from the Umbrel App Store like any other app — see [Umbrel](05-umbrel.md). The App Store carries a new version once it has been submitted there, which can be later than the release it names.

## Desktop app

The desktop app checks for updates each time it starts and offers to install one and restart. There is nothing to run.

## Deltabadger.com hosting

Updates are installed for you. The in-app notice stays quiet.

## Turning the check off

`DELTABADGER_UPDATE_CHECK=false` stops it, and the Settings section then only shows your version.

The check is the only outbound request an otherwise offline install makes: an unauthenticated `GET` to `api.github.com` for the tag of the latest release, twice a day. It sends no version, no identifier and nothing about your install, and it is never made at all on the desktop app or on Deltabadger.com hosting.
