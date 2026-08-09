# Says so at startup when this instance holds encrypted values its configured keys cannot
# read. Worth naming: a value that fails to decrypt comes back as its own ciphertext rather
# than raising, so nothing looks wrong until a credential appears as a base64 blob.
#
# Warns rather than refusing to start, for the reason recorded in secret_key_base_guard.rb: an
# instance in this state still holds recoverable data and is the one that needs
# deltabadger:encryption:report and :reset. Refusing would also deny db:migrate on a schema old
# enough that the scan below queries a table it has not created yet, and would reach the
# desktop build as nothing more than "application failed to start".
module EncryptionKeyCheck
  WARNING = <<~MESSAGE.freeze
    ============================== ENCRYPTION KEY NOTICE =============================
    Some stored values cannot be read with the encryption keys this instance is
    configured with. They were written under different ones.

    Anything not yet saved over is intact, and a correct key still reads it. This
    cannot tell which of these values have already been written since — saving over one
    replaces it for good — so restore the right key before changing any credential here,
    and before leaving this instance running.

    The usual causes:

      * #{EncryptionKeys::PRIMARY_KEY_VAR} or
        #{EncryptionKeys::SALT_VAR} set to something other
        than the values that wrote this data. Restore them and start again.
      * SECRET_KEY_BASE replaced on an instance that derives its encryption keys from
        it. Restore the previous value and start again.
      * More than one container on this volume running with different values. They all
        need the same ones.

    To see what is stored:
      docker compose run --rm --no-deps deltabadger rake deltabadger:encryption:report

    If the previous values are gone for good, the credentials cannot be recovered and
    must be replaced at their source. See the README section
    "Moving to a new SECRET_KEY_BASE".
    ==================================================================================
  MESSAGE

  # :readable | :unreadable | :indeterminate
  def self.readability(logger: Rails.logger)
    EncryptedCredentials::Report.new.data_written_under_another_key? ? :unreadable : :readable
  rescue StandardError => e
    # Could not tell. On a database that is not prepared yet this is the normal state and means
    # nothing is wrong, so it is a log line rather than a banner — but it is not reported as
    # health either.
    logger.info("Encryption key check did not complete: #{e.class}") if logger
    :indeterminate
  end

  def self.emit_warning!(logger: Rails.logger)
    return nil unless readability(logger: logger) == :unreadable

    logger.error(WARNING) if logger
    warn(WARNING)
    WARNING
  end
end

# Deferred: the check reads through the models, which are not loaded while initializers run.
# Skipped during asset precompilation, which Rails runs with a generated dummy secret.
Rails.application.config.after_initialize do
  if Rails.env.production? && ENV['SECRET_KEY_BASE_DUMMY'].blank?
    EncryptionKeyCheck.emit_warning!
  end
end
