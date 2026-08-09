# Key material for ActiveRecord Encryption.
#
# Both values may be supplied directly. When they are not, they are derived from
# secret_key_base, which is what every install did before these variables existed — so an
# install that sets neither behaves byte-for-byte as it always has and needs no migration.
#
# Supplying them decouples stored data from the session secret. The two have opposite
# lifecycles: replacing a session secret should cost a re-login, while replacing a data key
# costs re-encrypting everything. Derived from one value they move together, so changing the
# session secret makes every stored credential unreadable and locks out anyone using
# two-factor.
#
# The derivation below must not change. It is the only way an operator holding an older
# secret can recompute the key that wrote their data.
module EncryptionKeys
  class ConfigurationError < StandardError; end

  PRIMARY_KEY_VAR = 'ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY'
  SALT_VAR = 'ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT'

  # Written into the secrets file when an install is first set up with keys supplied through
  # its environment. Without it, losing those later looks identical to an install that never
  # had them: both variables absent is a valid state, so the app would quietly derive a
  # different pair from the stored SECRET_KEY_BASE and read nothing.
  EXTERNAL_MARKER_VAR = 'ACTIVE_RECORD_ENCRYPTION_KEYS_EXTERNAL'

  def self.derived_primary_key(secret)
    Digest::SHA256.hexdigest("#{secret}-ar-encryption-primary")
  end

  def self.derived_salt(secret)
    Digest::SHA256.hexdigest("#{secret}-ar-encryption-salt")
  end

  def self.independent?(env = ENV)
    env[PRIMARY_KEY_VAR].presence.present? && env[SALT_VAR].presence.present?
  end

  def self.partially_configured?(env = ENV)
    [env[PRIMARY_KEY_VAR].presence, env[SALT_VAR].presence].compact.size == 1
  end

  # A key used against a different salt decrypts nothing. support_unencrypted_data is on, so
  # that failure does not raise on read — the value comes back as its own ciphertext, and a
  # later write to that attribute stores it re-encrypted, past recovery even with the
  # original secret. Refusing to start is the last point at which that is preventable.
  #
  # Safe to raise on, unlike SecretKeyBaseGuard: these are configurations an operator has just
  # introduced, not a state an existing install is stranded in. Raising does stop every
  # environment-loading command, including deltabadger:encryption:derived_keys, so the
  # messages carry what is needed rather than naming a command that cannot run.
  def self.validate!(env = ENV, secret:)
    if env[EXTERNAL_MARKER_VAR].presence && !independent?(env)
      raise ConfigurationError, <<~MESSAGE
        This install was set up with its own encryption keys, supplied through its
        environment, and they are not set now.

        Restore #{PRIMARY_KEY_VAR} and
        #{SALT_VAR} to the values this install was created with.
        Nothing else reads its data. They are not in the secrets file — that file only
        records that they came from elsewhere.

        Starting without them would not fail on its own: it would derive a different pair
        from SECRET_KEY_BASE and quietly read nothing, which is why this stops instead.

        If those values are gone for good, remove the #{EXTERNAL_MARKER_VAR} line from the
        secrets file. This install will then start, and its stored credentials will need
        replacing at their source.
      MESSAGE
    end

    return nil unless partially_configured?(env)

    missing = env[PRIMARY_KEY_VAR].presence ? SALT_VAR : PRIMARY_KEY_VAR
    present = missing == SALT_VAR ? PRIMARY_KEY_VAR : SALT_VAR

    raise ConfigurationError, <<~MESSAGE
      #{present} is set but #{missing} is not.

      These two are a pair. A key used against a different salt cannot read anything this
      install has stored, so it will not start on half of one.

      If this install was already using its own encryption keys and one has gone missing,
      put the ORIGINAL #{missing} back. Only that value reads your data. Recover it from
      wherever this install's environment is configured, or from a backup of it.

      Do NOT substitute the values below in that case. They are derived from SECRET_KEY_BASE
      and are the ones an install that has NEVER used its own keys is currently encrypting
      with. Using them on an install that has is what would make its data unreadable.

      If you were part-way through setting these for the first time, the matching pair is:

        #{PRIMARY_KEY_VAR}=#{derived_primary_key(secret)}
        #{SALT_VAR}=#{derived_salt(secret)}

      If you meant to keep deriving them from SECRET_KEY_BASE, remove #{present} instead.

      If the original is gone for good, remove #{present} as well. The data it encrypted is
      unreadable either way, and removing it at least lets this install start so that
      `rake deltabadger:encryption:report` and `:reset` can run.
    MESSAGE
  end

  def self.primary_key(env = ENV, secret:)
    env[PRIMARY_KEY_VAR].presence || derived_primary_key(secret)
  end

  def self.key_derivation_salt(env = ENV, secret:)
    env[SALT_VAR].presence || derived_salt(secret)
  end

  # Whether configured values were taken from this secret rather than generated
  # independently. Values taken from it are exactly as recoverable as it is, which is what
  # SecretKeyBaseGuard needs before telling an operator whether stored data is exposed.
  def self.derived_from_secret?(env = ENV, secret:)
    return false unless independent?(env)

    env[PRIMARY_KEY_VAR] == derived_primary_key(secret) && env[SALT_VAR] == derived_salt(secret)
  end
end

Rails.application.configure do
  secret = Rails.application.secret_key_base

  EncryptionKeys.validate!(secret: secret)

  config.active_record.encryption.primary_key = EncryptionKeys.primary_key(secret: secret)
  config.active_record.encryption.key_derivation_salt = EncryptionKeys.key_derivation_salt(secret: secret)

  # Allow reading data that was not yet encrypted (for migration transition)
  config.active_record.encryption.support_unencrypted_data = true
end
