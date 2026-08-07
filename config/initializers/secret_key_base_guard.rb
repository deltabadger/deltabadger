# secret_key_base is the sole root of ActiveRecord Encryption in this app (see
# config/initializers/active_record_encryption.rb), so a weak one means every stored
# exchange credential is decryptable and session cookies are forgeable. So warn loudly
# on every production boot; see the note above the check below for why this must not abort.
module SecretKeyBaseGuard
  # EXACT matches only. A substring match would abort every Umbrel container, which
  # legitimately runs on "${APP_SEED}-secret-key-base" (deltabadger/docker-compose.yml:24).
  #
  # The first three are every literal SECRET_KEY_BASE value this repository has ever
  # shipped, from a sweep of every blob in its history: "dev-secret-key-not-for-production"
  # and "your_secret_key_base_here" in .env.docker.example, "placeholder" in the Dockerfile's
  # asset-precompile step. Length is not a substitute for this list —
  # "your_secret_key_base_here" is 25 characters, so without an entry the guard would call
  # it merely short and tell its operator that nothing had leaked.
  #
  # "-secret-key-base" is what deltabadger/docker-compose.yml expands to when APP_SEED is
  # unset, which is any use of that file outside Umbrel. A real Umbrel secret is
  # "<seed>-secret-key-base" and is unaffected, because the match is exact. That file
  # supplies the value through environment:, not .env.docker, which is why PUBLISHED_WARNING
  # step 6 names both places.
  KNOWN_PLACEHOLDERS = %w[
    dev-secret-key-not-for-production
    your_secret_key_base_here
    placeholder
    -secret-key-base
    changeme
    secret
  ].freeze

  # Deliberately below the 128 chars `openssl rand -hex 64` produces: the bar is
  # "not obviously a typed-in word", not "matches our preferred generator". Raising it
  # would lock out existing installs whose working secret is merely shorter than ours.
  MINIMUM_LENGTH = 32

  # This message is for the one case that is an actual incident: the secret is a value
  # published in this repository, so every install that ever copied it shares a key that
  # anybody can read. The recipe below is deliberately clear-and-reissue rather than
  # re-encryption: re-encrypting in place would preserve credentials that are already
  # exposed in any leaked copy of the database, which does not fix anything.
  PUBLISHED_WARNING = <<~MESSAGE.freeze
    ================================ SECURITY WARNING ================================
    SECRET_KEY_BASE is a value published in the public repository.

    It is the encryption key for EVERYTHING encrypted in this instance: exchange API
    keys, your two-factor secret, withdrawal addresses, and market-data configuration.
    Anyone with a copy of this database can already read every credential in it. The
    same value also signs and encrypts session cookies, so a reachable instance can be
    signed into without ever copying the database. Deleting a credential here does NOT
    invalidate it — anything held by an exchange or another service must be revoked
    there. The exception is this instance's own REST API and MCP tokens: they exist
    only in this database, so step 5 does invalidate those.

    The commands below use the service name from docker-compose.yml. The Umbrel app
    definition calls that service web — substitute it if you run from that file.

    In this order:

      1. Stop the app.  docker compose stop
      2. Back up your data, now that nothing is writing to it. The default deployment
         uses a named volume, not a host directory:
           docker compose cp deltabadger:/app/storage ./storage-backup
         If you changed to a bind mount, copy that host directory instead. On a
         default install this copy includes /app/storage/.secrets — the key
         itself — so it decrypts itself. Keep it as carefully as the database.
      3. List what is stored:
           docker compose run --rm --no-deps deltabadger \\
             rake deltabadger:encryption:report
         Copy your withdrawal addresses — they are not recoverable afterwards.
      4. REVOKE everything it listed, at its source: exchange API keys, fee keys, SMTP
         password, Alpaca key and secret, CoinGecko key, market-data token. Revoke
         only — replacements have nowhere to live until step 7.
      5. Clear what is stored:
           docker compose run --rm --no-deps -e CONFIRM=clear-credentials \\
             deltabadger rake deltabadger:encryption:reset
         This also deletes your REST API token and every MCP client's token, which is
         what actually invalidates them: they are bearer tokens stored in the clear
         and derived from nothing, so changing the secret alone leaves them trading.
      6. Point this install at a new key. Blank the SECRET_KEY_BASE line in
         .env.docker, then delete the generated key, a separate file in the volume:
           docker compose run --rm --no-deps deltabadger \\
             rm -f /app/storage/.secrets
         On a default install .env.docker is already blank and .secrets is where
         your key actually is, so this deletion is what rotates anything at all.
         A new one is written only when that file is absent; an existing one is
         never overwritten.
         If your value comes from a compose file instead — the Umbrel app
         definition sets it from APP_SEED — put a freshly generated value there
         rather than blanking it:  openssl rand -hex 64
         That file runs web and jobs off one volume, and each container generates
         its own key when it finds none, so blanking would split them onto two.
      7. Recreate the container so it picks up the edited value:
           docker compose up -d --force-recreate
         Starting the stopped container instead would bring it back with the old
         value still baked in: compose reads the environment when a container is
         created, not when it starts. A strong per-install secret is generated
         for you.
      8. Sign in, issue fresh credentials, add them, re-enable two-factor, re-issue
         your REST API token, reconnect your MCP clients, and restart your bots and
         rules — the reset cleared and stopped all of them.

    Interactive Brokers is the slow one: its key pair is generated by this app and stored
    here, so it can only be replaced after step 7 — run the connect wizard again, register
    the new key in IBKR's portal, and wait for them to activate it.

    Do NOT just change the value on its own — every encrypted field derives from it, so
    that alone makes your two-factor secret unreadable and locks you out.
    ==================================================================================
  MESSAGE

  # Reached by a secret that is merely SHORT. "Short" is all we actually know: the check is
  # length, and a random 31-character value and the word "password1" both land here. So this
  # must not assert either way. It cannot say the data has leaked — that would send an
  # operator whose secret never left the machine through a recovery that destroys IBKR
  # credentials taking days to replace. It also cannot say nothing leaked, which is what it
  # used to say: absence from KNOWN_PLACEHOLDERS is not evidence a value was randomly
  # generated. So it states the risk, and hands the operator the one test they can apply and
  # we cannot — did a person choose this, and has it been anywhere public.
  SHORT_WARNING = <<~MESSAGE.freeze
    ================================ SECURITY NOTICE =================================
    SECRET_KEY_BASE is shorter than #{MINIMUM_LENGTH} characters.

    It is the encryption key for everything encrypted in this instance — exchange API
    keys, your two-factor secret, withdrawal addresses — and it also signs and encrypts
    session cookies. A short key is cheap to brute-force offline from a single encrypted
    value or one captured cookie, and recovering it yields both the stored credentials
    and the ability to sign in as you.

    Whether that is a live incident depends on where the value came from, which cannot
    be told from the value itself. Only you know:

      * If a person chose it, or it has ever appeared anywhere public — a repository,
        an issue, a paste, a screenshot, a support thread — treat it as compromised.
        Follow the full procedure in the README under "Moving to a new SECRET_KEY_BASE",
        including revoking every credential at its source, and do it now.
      * If it was randomly generated and has never left this machine, nothing here is
        known to be exposed. Move to a stronger secret at your convenience, using that
        same procedure.

    Either way you cannot simply change the value: every encrypted field derives from it,
    so changing it alone makes your two-factor secret unreadable and locks you out.
    ==================================================================================
  MESSAGE

  # Published in this repository, so every install that copied it shares one key that
  # anybody can read. This is a compromise, not a weakness.
  def self.published?(secret)
    KNOWN_PLACEHOLDERS.include?(secret.to_s)
  end

  # Shorter than we would like. Says nothing about how the value was chosen — see
  # SHORT_WARNING for why that distinction is left to the operator rather than guessed here.
  def self.short?(secret)
    secret.to_s.length < MINIMUM_LENGTH
  end

  def self.weak?(secret)
    published?(secret) || short?(secret)
  end

  def self.warning_for(secret)
    return PUBLISHED_WARNING if published?(secret)
    return SHORT_WARNING if short?(secret)

    nil
  end

  # Extracted from the boot branch below purely so it can be tested. Left inline, a
  # regression that swapped warning_for back to weak? + PUBLISHED_WARNING would pass every
  # test and quietly tell an operator with a short private secret that their database is
  # readable by anyone.
  #
  # `if logger` rather than `logger&.error` — no behavioral difference on this single call,
  # but safe navigation on a logger elsewhere in this codebase has previously produced a
  # silent no-op, so the explicit conditional is preferred here as a matter of habit. Being
  # explicit costs nothing.
  def self.emit_warning!(secret, logger: Rails.logger)
    warning = warning_for(secret)
    return nil if warning.nil?

    logger.error(warning) if logger # rubocop:disable Style/SafeNavigation -- see comment above
    warn(warning)
    warning
  end
end

# WARN, do not abort. An existing install running on the weak secret still has
# readable data; aborting its boot would strand it with no way to migrate.
#
# Skipped during `assets:precompile`, which Rails runs with a generated dummy secret.
# rubocop:disable Style/IfUnlessModifier -- block form keeps `if Rails.env.production?`
# ahead of the emit_warning! call, which test/config/secret_key_base_guard_test.rb
# relies on to locate this branch by source text.
if Rails.env.production? && ENV['SECRET_KEY_BASE_DUMMY'].blank?
  SecretKeyBaseGuard.emit_warning!(Rails.application.secret_key_base)
end
# rubocop:enable Style/IfUnlessModifier
