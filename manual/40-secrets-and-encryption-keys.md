# Secrets and encryption keys

Secrets are auto-generated on first run and stored in `/app/storage/.secrets` (inside the volume). These persist across container restarts and upgrades.

## Replacing SECRET_KEY_BASE without losing data

An install that sets `ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY` and `ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT` keeps its stored data encrypted under those, not under `SECRET_KEY_BASE`. Replacing `SECRET_KEY_BASE` there ends every session and leaves stored data untouched — no credentials to re-enter, no two-factor to re-enrol. Password reset and confirmation emails already sent stop working, and your REST API and MCP tokens are unaffected, so revoke those separately if the old value was ever exposed.

Installs created from scratch after these keys existed already have them. To check:

```bash
docker compose run --rm --no-deps deltabadger \
  rake deltabadger:encryption:derived_keys
```

If your install does not have them yet, that command prints the two values it currently derives. Put both wherever this install's environment comes from, recreate the container with `docker compose up -d --force-recreate`, and confirm it starts and your credentials still read. `SECRET_KEY_BASE` is free to change from then on.

If your compose file defines more than one service, set them in **every** service block. They share one database, and different values leave each unable to read the other's writes.

Only ever set them to the values that command prints. Any others make stored data unreadable.

## Moving to a new SECRET_KEY_BASE

`SECRET_KEY_BASE` is the encryption key for everything encrypted in this instance — exchange API keys, your two-factor secret, withdrawal addresses, and market-data configuration. You cannot simply change it on a running install: every encrypted field is derived from it, so changing the value in place makes all of them unreadable and locks you out. The steps below clear the stored credentials under the old key and let you re-enter them under the new one.

**Who needs step 4.** Treat your credentials as compromised, and step 4 (revoke) as not optional, if either of these is true:

- This install's `SECRET_KEY_BASE` was ever one of the placeholder values shipped in this repository, or any other value that has appeared somewhere public. Anyone holding a copy of your database can already read every credential in it.
- A person chose the value rather than generating it randomly. A short or human-chosen key is cheap to brute-force offline from a single encrypted database value or one captured session cookie, which yields the same result. A value not being on our placeholder list says nothing about how it was chosen, and neither check below can see how it was chosen either — add `-e PUBLISHED=yes` to the commands in steps 3 and 5 so they say so too.
- You have already changed `SECRET_KEY_BASE` and can no longer sign in. The stored credentials were written under the *previous* key, and the strong value you are running now tells you nothing about whether that previous one was published.

Only if your secret was randomly generated and has never left the machine is nothing here known to be exposed. Then follow the same steps at your convenience and skip step 4. If you are unsure, revoke.

The report and the reset make this call where they can, and say which way they went: they treat the data as compromised if the secret currently in use is one published in this repository, or if anything stored will not decrypt under it — the signature of that third case. Neither check can see how a value was chosen, nor a key you have already replaced and discarded. Those two are your call to make: add `-e PUBLISHED=yes` to the report and reset commands to force the compromised wording.

Follow this order exactly. Back up only after stopping the app — a copy taken while it is still writing is not consistent, and it is the only way back. Run the report before revoking anything — it is what tells you which credentials exist; revoking first means working from memory and missing the fee keys, SMTP password, or market-data token. Revoke before the reset, and do not reissue anything until after the restart — a replacement has nowhere to be stored until the app is running again, and for Interactive Brokers, registering a new key early hands them a public key whose private half the reset then deletes.

The commands below use the service name from `docker-compose.yml`. The Umbrel app definition calls that service `web` — substitute it if you run from that file.

1. Stop the app.  `docker compose stop`
2. Back up your data, now that nothing is writing to it. The default deployment uses a named volume, not a host directory:
   ```bash
   docker compose cp deltabadger:/app/storage ./storage-backup
   ```
   If you changed `docker-compose.yml` to use a bind mount, copy that host directory instead. On a default install this copy includes `/app/storage/.secrets` — the key itself — so the backup decrypts itself. Keep it as carefully as the database, and do not put it anywhere shared. If your key comes from `.env.docker` or a compose file it is *not* in this copy, and step 6 overwrites it — save that value too, or this backup cannot be restored.
3. List what is stored:
   ```bash
   docker compose run --rm --no-deps deltabadger \
     rake deltabadger:encryption:report
   ```
   Copy your withdrawal addresses — they are not recoverable afterwards.
4. **REVOKE everything it listed, at its source: exchange API keys, fee keys, SMTP password, Alpaca key and secret, CoinGecko key, market-data token.** Revoke only — replacements have nowhere to live until step 7.
5. Clear what is stored:
   ```bash
   docker compose run --rm --no-deps -e CONFIRM=clear-credentials \
     deltabadger rake deltabadger:encryption:reset
   ```
6. Point this install at a new key. Blank the `SECRET_KEY_BASE` line in `.env.docker`, then delete the generated key, which is a separate file inside the volume:
   ```bash
   docker compose run --rm --no-deps deltabadger \
     rm -f /app/storage/.secrets
   ```
   On a default install `.env.docker` is already blank and `.secrets` is where your key actually is, so this deletion is what rotates anything at all. A new one is written only when that file is absent; an existing one is never overwritten.

   If your value comes from a compose file instead — the Umbrel app definition sets it from `APP_SEED` — put a freshly generated value in **both service blocks** rather than blanking it:
   ```bash
   openssl rand -hex 64
   ```
   That file sets `SECRET_KEY_BASE` twice, once under `web` and once under `jobs`, and each container's own environment beats the generated file. Use the same value in both: change one and the two halves of the app run on different keys, and neither can read what the other wrote.
7. Recreate the container so it picks up the edited value:
   ```bash
   docker compose up -d --force-recreate
   ```
   Starting the stopped container instead would bring it back with the old value still baked in: Compose reads `env_file` and `environment:` when a container is *created*, not when it starts, so editing the file does not reach a container that already exists. A strong per-install secret is generated for you.
8. Sign in, issue fresh credentials, add them, re-enable two-factor, re-issue your REST API token, reconnect your MCP clients, and restart your bots and rules — the reset cleared and stopped all of them.

Your REST API token and every connected MCP client are cleared by step 5, not by you in step 4, and the reset prints how many it deleted. They are bearer tokens held in this database and derived from nothing, so changing `SECRET_KEY_BASE` leaves them working and still able to place orders — deleting them is what invalidates them. The OAuth client registrations themselves survive, so a client can reconnect under the client ID it already has, but it has to be authorized again.

Interactive Brokers is the slow one: its key pair is generated by this app and stored here, so it can only be replaced after step 7 — run the connect wizard again, register the new key in IBKR's portal, and wait for them to activate it. Registering a fresh key any earlier would hand IBKR a public key whose private half step 5 then deletes.
