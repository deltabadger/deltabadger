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
         If your key comes from .env.docker or a compose file it is NOT in this
         copy, and step 6 overwrites it: save that value too, or this backup
         cannot be restored.
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
         If you set ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY and
         ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT — anywhere: that file,
         .env.docker, or a compose file — remove both lines as well. They override
         the new key, so leaving them in place means the credentials you add at
         step 8 are encrypted under the same one you are replacing. No pair is
         written for you here, since that happens only for an install with no
         database, so once you are back up run
         `rake deltabadger:encryption:derived_keys` and set what it prints BEFORE
         step 8. Skip that and this install goes back to deriving them from
         SECRET_KEY_BASE, with the same exposure the next time it changes.
         If your value comes from a compose file instead — the Umbrel app
         definition sets it from APP_SEED — put a freshly generated value in
         both service blocks rather than blanking it:  openssl rand -hex 64
         That file sets the key twice, under web and under jobs, and each
         container's own environment beats the generated file. Use the same
         value in both: change one and they run on different keys, and neither
         can read what the other wrote.
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

  # Reached when the secret is weak but the encryption keys are kept separately, so nothing
  # stored derives from it. Sessions are still forgeable, which is enough to be signed in as
  # the operator — but the stored credentials are not readable from this value, and sending
  # someone to revoke every exchange key over it would be wrong.
  PUBLISHED_SESSIONS_WARNING = <<~MESSAGE.freeze
    ================================ SECURITY WARNING ================================
    SECRET_KEY_BASE is a value published in the public repository.

    This instance keeps its encryption keys separately, so what is stored here does not
    derive from this value and is not readable from it. What this value does do is sign
    and encrypt session cookies: anyone who can reach this instance can sign into it as
    you, without needing the database.

    Replace it. Because the encryption keys are kept separately, stored data is
    unaffected:

      1. Revoke this instance's own REST API token and every connected MCP client, under
         Settings. They are bearer tokens that do not derive from this value, so
         replacing it does not end them, and anyone who signed in as you could have
         issued one.
      2. Put a fresh value in place of the published one, wherever this install takes it
         from — /app/storage/.secrets, .env.docker, or your compose file:
           openssl rand -hex 64
         Leave the two encryption key variables exactly as they are.
      3. Recreate the container:
           docker compose up -d --force-recreate
      4. Sign in again. Every session ends, which is the point. Any password reset or
         confirmation email already sent stops working — request a new one.
    ==================================================================================
  MESSAGE

  # As above, for a secret that is merely short. See SHORT_WARNING for why this cannot assert
  # whether the value ever leaked.
  SHORT_SESSIONS_WARNING = <<~MESSAGE.freeze
    ================================ SECURITY NOTICE =================================
    SECRET_KEY_BASE is shorter than #{MINIMUM_LENGTH} characters.

    This instance keeps its encryption keys separately, so stored credentials do not
    derive from this value. It still signs and encrypts session cookies, and a short one
    is cheap to brute-force offline from a single captured cookie, which yields the
    ability to sign in as you.

    Replacing it leaves stored data untouched. Put a fresh value wherever this install
    takes it from, leaving the encryption key variables as they are:
      openssl rand -hex 64
    then `docker compose up -d --force-recreate` and sign in again. Any password reset
    or confirmation email already sent stops working.

    If this value has ever been anywhere public, also revoke this instance's REST API
    token and its connected MCP clients: those do not derive from it and outlive the
    replacement.
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

  # Whether a weak secret also exposes what is stored. True when the encryption keys derive
  # from it, and equally true when they were taken from it — those are exactly as recoverable
  # as the secret itself. False only when they were generated independently, in which case a
  # weak secret costs sessions rather than credentials.
  def self.data_exposed_by?(secret, env = ENV)
    return true unless EncryptionKeys.independent?(env)

    EncryptionKeys.derived_from_secret?(env, secret: secret)
  end

  def self.warning_for(secret, env = ENV)
    exposed = data_exposed_by?(secret, env)

    return exposed ? PUBLISHED_WARNING : PUBLISHED_SESSIONS_WARNING if published?(secret)
    return exposed ? SHORT_WARNING : SHORT_SESSIONS_WARNING if short?(secret)

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
