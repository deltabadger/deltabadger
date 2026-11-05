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

Your data lives in the `deltabadger_data` volume, not in the container, so removing the old container keeps everything. The new one updates the data on start.

## Docker Compose

```bash
docker compose pull
docker compose up -d
```

`docker compose up -d` stops the old container and starts the new one. The compose file gives a running Deltabadger 60 seconds to finish what it is doing before it is stopped.

## Umbrel

Update Deltabadger from the Umbrel App Store like any other app — see [Umbrel](05-umbrel.md). The App Store carries a new version once it has been submitted there, which can be later than the release it names.

## Desktop app

The desktop app checks for updates when it starts and every 12 hours, and downloads an update in the background. The **Updates** row under **Settings → Account** says so, and its **Check for updates** button checks at once.

When an update has downloaded, a banner says **An update is ready.** On a Mac:

- **Restart to update** installs it now. Deltabadger finishes its current work first, then installs the update and opens again by itself.
- Or leave it for later: choose **Quit** and Deltabadger installs the update before it closes. The next start runs the new version.

If the new version needs to update your data, the first start after the update saves a copy of it first, in the data folder's `db/backups/` (see [Data and backups](30-data-and-backups.md#copies-deltabadger-makes)), so that start can take a few minutes.

An update marked important says so in the banner: **An important update is ready. Please restart soon to install it.**

**With the `.deb` package**, the app tells you about an update but doesn't download it: **An update is available. Quit Deltabadger, then download the new .deb file from the download page and open it to install it.**

**When the banner says Deltabadger can't install it itself**, the app is running from somewhere it can't replace itself. On a Mac this happens when it is opened straight from the downloaded disk image or from the Downloads folder: quit it, move it into **Applications**, and open it from there. It also happens when the folder `/Users/Shared/.deltabadger`, which Deltabadger uses so that it is never replaced while it runs in another account on this Mac, can no longer be written by every account. Otherwise, download Deltabadger from the download page and install it again over the old one; your data stays where it is.

The banner's other messages say what went wrong and what to do. A download that failed, or didn't verify, is tried again later. **The update couldn't be saved because the disk is full** needs some free space first; then press **Try again**.

The download page is the [releases page](https://github.com/deltabadger/deltabadger/releases).

### Going back to an earlier version

There is no automatic way back. If an update leaves you stuck, contact support. The way back they will walk you through: install the earlier version from the releases page, quit it, and put the two files from the `before-update-` copy that update made back into the data folder's `db/` folder (see [Data and backups](30-data-and-backups.md#restoring-a-copy-deltabadger-made)). An earlier version refuses data a newer one has already updated, which is why the copy is needed.

## Deltabadger.com hosting

Updates are installed for you, including the update to 3.0. There is nothing to do. The in-app notice stays quiet.

## Updating from 2.x

Deltabadger 3 takes a 2.x install over on its first start: you update the way you always have, and it continues with the same accounts, bots, keys and history.

On that first start it:

1. waits, if your previous version stopped less than about two minutes ago, until it has surely shut down. Meanwhile the app's address shows **Deltabadger is starting** and reloads on its own;
2. saves a copy of your data in `backups/pre-3.0-<date and time>` next to the database (`/app/storage/backups/` in Docker). The log says **Saved a copy of your data to … before updating it.** Starting again without any change in between adds no second copy;
3. updates the data, and starts your bots.

The copy takes as long as copying your database files does. Until the first start has finished, the address may show the waiting page or nothing; with Docker Compose the container reports healthy once the app answers.

### If it doesn't start

Nothing trades while Deltabadger is refused. Its address shows a line saying what is wrong and what to do; the same line is in the log (`docker compose logs deltabadger`). The container stays up showing it and reports unhealthy. Fix the cause, then restart it with `docker compose restart` (or `docker restart deltabadger`).

| The line says | What to do |
|---|---|
| this data is from an older version of Deltabadger | Your 2.x install never finished an update. Bring it up to date with 2.70.21 once, then start 3 again; the steps are below. Nothing was changed. |
| your previous version of Deltabadger still seems to be running on this data | Another container, an old one or one under another name, still runs 2.x on the same volume. Stop it, then start this again. Nothing was changed. |
| it couldn't save a copy of your data to … first | The copy failed, because the disk is full or the folder can't be written to. Free some space or fix the folder, then start again. Your data hasn't been changed. |
| can't read this install with this `SECRET_KEY_BASE` | The secret in your environment is not the one this install was made with. Use the original one (see [Secrets and encryption keys](31-secrets-and-encryption-keys.md)). |
| some of your data has to be checked by support first | Contact support. Nothing was bought or sold. |

#### An install from an older version

A 2.x install that skipped versions, or whose last update was interrupted, has to be brought up to date by 2.70.21, the last 2.x version, before 3 can take it over. With Docker Compose:

1. Stop Deltabadger:
   ```bash
   docker compose stop
   ```
2. In `docker-compose.yml`, change the `image:` line to `ghcr.io/deltabadger/deltabadger:2.70.21`.
3. Update the data:
   ```bash
   docker compose run --rm deltabadger migrate
   ```
4. Change the `image:` line back to `ghcr.io/deltabadger/deltabadger:latest` and start it:
   ```bash
   docker compose up -d
   ```

With the single `docker run` container, step 3 is `docker run --rm -v deltabadger_data:/app/storage ghcr.io/deltabadger/deltabadger:2.70.21 migrate`, after `docker stop deltabadger`; then `docker start deltabadger`.

### Going back to 2.x

`handback` returns an install to 2.70.21 with everything 3 recorded, including the orders it placed. Stop Deltabadger first:

```bash
docker compose stop
docker compose run --rm deltabadger handback
```

It ends with **handed back: … bot(s) scheduled. You can start Deltabadger 2.70.21 now.** Change the `image:` line to `ghcr.io/deltabadger/deltabadger:2.70.21` and run `docker compose up -d`. Starting 3 again later takes the install over again.

`handback` refuses, saying why, while 3 holds something 2.x can't take, and for an install Deltabadger 3 created new (there is no earlier version to return it to). For restoring the copy made before the takeover instead, see [Data and backups](30-data-and-backups.md#restoring-a-copy-deltabadger-made).

## Turning the check off

`DELTABADGER_UPDATE_CHECK=false` stops it, and the Settings section then only shows your version.

The check is the only outbound request an otherwise offline server install makes: an unauthenticated `GET` to `api.github.com` for the tag of the latest release, twice a day. It sends no version, no identifier and nothing about your install, and it is never made on Deltabadger.com hosting. The desktop app doesn't make it either: its own update check, above, reads the latest release's update file from GitHub.
