# secret_key_base is the sole root of ActiveRecord Encryption in this app (see
# config/initializers/active_record_encryption.rb), so a weak one means every stored
# exchange credential is decryptable and session cookies are forgeable. So warn loudly
# on every production boot; see the note above the check below for why this must not abort.
module SecretKeyBaseGuard
  # EXACT matches only. A substring match would abort every Umbrel container, which
  # legitimately runs on "${APP_SEED}-secret-key-base" (deltabadger/docker-compose.yml:24).
  KNOWN_PLACEHOLDERS = %w[
    dev-secret-key-not-for-production
    placeholder
    changeme
    secret
  ].freeze

  # Deliberately below the 128 chars `openssl rand -hex 64` produces: the bar is
  # "not obviously a typed-in word", not "matches our preferred generator". Raising it
  # would lock out existing installs whose working secret is merely shorter than ours.
  MINIMUM_LENGTH = 32

  # This message is read by self-hosters in a terminal. It must NOT hand them a
  # rotation recipe until one has actually been built and exercised — a half-right
  # procedure here costs them their 2FA access.
  WARNING = <<~MESSAGE
    ================================ SECURITY WARNING ================================
    SECRET_KEY_BASE is blank, too short, or a value published in the public repository.

    It is the encryption key for EVERYTHING encrypted in this instance: exchange API
    keys, your two-factor secret, withdrawal addresses, and market-data configuration.
    Anyone who obtains a copy of your database can read all of it.

    A NEW install fixes this by leaving SECRET_KEY_BASE empty in .env.docker — the
    container then generates and stores a strong one.

    On an EXISTING install, do NOT simply change the value. Every encrypted field is
    derived from it, so changing it makes your two-factor secret unreadable and will
    lock you out of this instance. A supported migration is not available yet; see
    https://github.com/deltabadger/deltabadger for the current guidance before
    attempting one.
    ==================================================================================
  MESSAGE

  def self.weak?(secret)
    secret = secret.to_s
    return true if secret.length < MINIMUM_LENGTH

    KNOWN_PLACEHOLDERS.include?(secret)
  end
end

# WARN, do not abort. An existing install running on the weak secret still has
# readable data; aborting its boot would strand it with no way to migrate.
#
# Skipped during `assets:precompile`, which Rails runs with a generated dummy secret.
if Rails.env.production? && ENV['SECRET_KEY_BASE_DUMMY'].blank?
  if SecretKeyBaseGuard.weak?(Rails.application.secret_key_base)
    Rails.logger.error(SecretKeyBaseGuard::WARNING)
    warn(SecretKeyBaseGuard::WARNING)
  end
end
