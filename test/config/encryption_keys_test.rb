# test/config/encryption_keys_test.rb
require 'test_helper'
require 'open3'

# Resolution is pure and tested by passing an env hash rather than mutating ENV or
# reconfiguring ActiveRecord::Encryption — global reconfiguration leaks across the parallel
# test workers. Same approach as SslConfigurationTest.
class EncryptionKeysTest < ActiveSupport::TestCase
  SECRET = 'a-secret-key-base-value'.freeze
  OTHER_SECRET = 'a-completely-different-secret'.freeze

  # Written out literally rather than by calling the module, so a change to the module cannot
  # silently redefine what "unchanged" means for an install that never opts in.
  LEGACY_PRIMARY = Digest::SHA256.hexdigest("#{SECRET}-ar-encryption-primary").freeze
  LEGACY_SALT = Digest::SHA256.hexdigest("#{SECRET}-ar-encryption-salt").freeze

  test 'with neither variable set it reproduces the legacy derivation exactly' do
    assert_equal LEGACY_PRIMARY, EncryptionKeys.primary_key({}, secret: SECRET)
    assert_equal LEGACY_SALT, EncryptionKeys.key_derivation_salt({}, secret: SECRET)
  end

  test 'with both variables set they are used verbatim' do
    env = { EncryptionKeys::PRIMARY_KEY_VAR => 'primary-from-env',
            EncryptionKeys::SALT_VAR => 'salt-from-env' }

    assert_equal 'primary-from-env', EncryptionKeys.primary_key(env, secret: SECRET)
    assert_equal 'salt-from-env', EncryptionKeys.key_derivation_salt(env, secret: SECRET)
  end

  # The whole point: the encryption key stops moving when the secret does.
  test 'configured keys do not move when the secret changes' do
    env = { EncryptionKeys::PRIMARY_KEY_VAR => 'primary-from-env',
            EncryptionKeys::SALT_VAR => 'salt-from-env' }

    assert_equal EncryptionKeys.primary_key(env, secret: SECRET),
                 EncryptionKeys.primary_key(env, secret: OTHER_SECRET)
    assert_equal EncryptionKeys.key_derivation_salt(env, secret: SECRET),
                 EncryptionKeys.key_derivation_salt(env, secret: OTHER_SECRET)
  end

  test 'without them both values move with the secret, which is the coupling being removed' do
    assert_not_equal EncryptionKeys.primary_key({}, secret: SECRET),
                     EncryptionKeys.primary_key({}, secret: OTHER_SECRET)
    assert_not_equal EncryptionKeys.key_derivation_salt({}, secret: SECRET),
                     EncryptionKeys.key_derivation_salt({}, secret: OTHER_SECRET)
  end

  test 'blank values are treated as absent' do
    env = { EncryptionKeys::PRIMARY_KEY_VAR => '', EncryptionKeys::SALT_VAR => '  ' }

    assert_equal LEGACY_PRIMARY, EncryptionKeys.primary_key(env, secret: SECRET)
    assert_not EncryptionKeys.independent?(env)
    assert_not EncryptionKeys.partially_configured?(env)
  end

  test 'independent? is true only when both are present' do
    assert EncryptionKeys.independent?(EncryptionKeys::PRIMARY_KEY_VAR => 'p',
                                       EncryptionKeys::SALT_VAR => 's')
    assert_not EncryptionKeys.independent?(EncryptionKeys::PRIMARY_KEY_VAR => 'p')
    assert_not EncryptionKeys.independent?({})
  end

  # A key against the wrong salt makes every stored value fail to decrypt. Because
  # support_unencrypted_data is on, that failure is silent: the read returns the raw
  # ciphertext and a later write to that attribute stores it re-encrypted, past recovery.
  # This combination must never reach a running app.
  test 'validate! raises when only the primary key is set' do
    error = assert_raises(EncryptionKeys::ConfigurationError) do
      EncryptionKeys.validate!({ EncryptionKeys::PRIMARY_KEY_VAR => 'p' }, secret: SECRET)
    end

    assert_includes error.message, EncryptionKeys::SALT_VAR
  end

  test 'validate! raises when only the salt is set' do
    error = assert_raises(EncryptionKeys::ConfigurationError) do
      EncryptionKeys.validate!({ EncryptionKeys::SALT_VAR => 's' }, secret: SECRET)
    end

    assert_includes error.message, EncryptionKeys::PRIMARY_KEY_VAR
  end

  # A partial pair stops every environment-loading command, including the task that would
  # tell the operator what to put back. So the message has to carry the values itself rather
  # than naming a command they cannot run.
  test 'the partial-pair message carries the derived values' do
    error = assert_raises(EncryptionKeys::ConfigurationError) do
      EncryptionKeys.validate!({ EncryptionKeys::PRIMARY_KEY_VAR => 'p' }, secret: SECRET)
    end

    assert_includes error.message, LEGACY_SALT
  end

  # An install that was ALREADY using its own keys and lost one must not be told to
  # substitute the derived pair: those are not the values encrypting its data, and using
  # them would make all of it unreadable. Restoring the original comes first.
  test 'the partial-pair message says to restore the original before offering derived values' do
    error = assert_raises(EncryptionKeys::ConfigurationError) do
      EncryptionKeys.validate!({ EncryptionKeys::PRIMARY_KEY_VAR => 'p' }, secret: SECRET)
    end

    restore_index = error.message.index('ORIGINAL')
    derived_index = error.message.index(LEGACY_SALT)

    assert restore_index, 'the message must tell the operator to restore the original value'
    assert restore_index < derived_index,
           'restoring the original must come before the derived values are offered'
    assert_match(/never used its own keys/i, error.message)
  end

  # Losing externally supplied keys must not look like an install that never had them:
  # both absent is otherwise a legitimate state, and the app would quietly derive a
  # different pair and read nothing.
  test 'validate! raises when the marker is set but the keys are gone' do
    error = assert_raises(EncryptionKeys::ConfigurationError) do
      EncryptionKeys.validate!({ EncryptionKeys::EXTERNAL_MARKER_VAR => 'true' }, secret: SECRET)
    end

    assert_match(/set up with its own encryption keys/i, error.message)
    assert_includes error.message, EncryptionKeys::EXTERNAL_MARKER_VAR
  end

  test 'validate! accepts the marker when the keys are present' do
    assert_nil EncryptionKeys.validate!(
      { EncryptionKeys::EXTERNAL_MARKER_VAR => 'true',
        EncryptionKeys::PRIMARY_KEY_VAR => 'p', EncryptionKeys::SALT_VAR => 's' }, secret: SECRET
    )
  end

  test 'validate! accepts both set and neither set' do
    assert_nil EncryptionKeys.validate!({}, secret: SECRET)
    assert_nil EncryptionKeys.validate!(
      { EncryptionKeys::PRIMARY_KEY_VAR => 'p', EncryptionKeys::SALT_VAR => 's' }, secret: SECRET
    )
  end

  # Tells the guard whether configured values still expose the data. Values taken FROM a
  # secret are exactly as recoverable as that secret.
  test 'derived_from_secret? recognises values taken from the current secret' do
    env = { EncryptionKeys::PRIMARY_KEY_VAR => LEGACY_PRIMARY,
            EncryptionKeys::SALT_VAR => LEGACY_SALT }

    assert EncryptionKeys.derived_from_secret?(env, secret: SECRET)
    assert_not EncryptionKeys.derived_from_secret?(env, secret: OTHER_SECRET)
  end

  test 'derived_from_secret? is false for independently generated values' do
    env = { EncryptionKeys::PRIMARY_KEY_VAR => SecureRandom.hex(32),
            EncryptionKeys::SALT_VAR => SecureRandom.hex(32) }

    assert_not EncryptionKeys.derived_from_secret?(env, secret: SECRET)
  end

  test 'derived_from_secret? is false when nothing is configured' do
    assert_not EncryptionKeys.derived_from_secret?({}, secret: SECRET)
  end

  # THE WIRING TESTS. These boot the app in a subprocess and read back what
  # ActiveRecord::Encryption is actually configured with. Asserting against the module
  # in-process proves nothing: with the variables cleared, an initializer that ignored
  # EncryptionKeys would compute the same fallback and pass. Only a boot with the variables
  # SET can tell the two apart.
  test 'a boot with both variables set uses them, and they do not move with the secret' do
    primary = SecureRandom.hex(32)
    salt = SecureRandom.hex(32)
    configured = { EncryptionKeys::PRIMARY_KEY_VAR => primary, EncryptionKeys::SALT_VAR => salt }

    first = booted_encryption_config(configured.merge('SECRET_KEY_BASE' => SecureRandom.hex(64)))
    second = booted_encryption_config(configured.merge('SECRET_KEY_BASE' => SecureRandom.hex(64)))

    assert_equal [primary, salt], first, 'the initializer ignored the configured keys'
    assert_equal first, second, 'the configured keys moved when the secret changed'
  end

  # The control. Without them the values still track the secret, which is what makes the
  # test above meaningful rather than vacuously true.
  test 'a boot with neither variable set still derives from the secret' do
    first = booted_encryption_config('SECRET_KEY_BASE' => SecureRandom.hex(64))
    second = booted_encryption_config('SECRET_KEY_BASE' => SecureRandom.hex(64))

    assert_not_equal first, second
  end

  # THE MONEY TEST, through the real model and the real attribute type.
  # with_encryption_context is block-scoped and thread-local, so unlike reconfiguring
  # globally it cannot leak into another test or worker.
  test 'a stored value survives a secret change when the keys are configured' do
    env = { EncryptionKeys::PRIMARY_KEY_VAR => SecureRandom.hex(32),
            EncryptionKeys::SALT_VAR => SecureRandom.hex(32) }

    record = with_keys(env, secret: SECRET) do
      AppConfig.create!(key: 'encryption_keys_probe', value: 'stored-value')
    end
    stored = raw_value(record.id)

    read_back = with_keys(env, secret: OTHER_SECRET) { AppConfig.find(record.id).value }

    assert_equal 'stored-value', read_back
    assert_equal stored, raw_value(record.id), 'reading must not rewrite the stored ciphertext'
  end

  # The failure this change removes. Note it does NOT raise: support_unencrypted_data returns
  # the ciphertext as though it were the value, which is why a wrong key is dangerous rather
  # than merely broken.
  test 'without configured keys a secret change makes the stored value unreadable' do
    record = with_keys({}, secret: SECRET) do
      AppConfig.create!(key: 'encryption_keys_probe', value: 'stored-value')
    end

    read_back = with_keys({}, secret: OTHER_SECRET) { AppConfig.find(record.id).value }

    assert_not_equal 'stored-value', read_back
    assert ActiveRecord::Encryption.encryptor.encrypted?(read_back),
           'the failed read should have returned the ciphertext, which is the hazard'
  end

  # Setting the values an install already derives has to be a no-op: data written before has
  # to read back after, or opting in would destroy everything.
  test 'setting the derived values reads back data written before they were set' do
    record = with_keys({}, secret: SECRET) do
      AppConfig.create!(key: 'encryption_keys_probe', value: 'stored-value')
    end
    pinned = { EncryptionKeys::PRIMARY_KEY_VAR => LEGACY_PRIMARY,
               EncryptionKeys::SALT_VAR => LEGACY_SALT }

    read_back = with_keys(pinned, secret: OTHER_SECRET) { AppConfig.find(record.id).value }

    assert_equal 'stored-value', read_back
  end

  private

  # Boots the app and reports what ActiveRecord::Encryption ended up configured with.
  def booted_encryption_config(env)
    script = 'print [ActiveRecord::Encryption.config.primary_key, ' \
             'ActiveRecord::Encryption.config.key_derivation_salt].join("|")'
    stdout, stderr, status = Open3.capture3(
      ENV.to_h.merge('RAILS_ENV' => 'test').merge(env),
      Rails.root.join('bin/rails').to_s, 'runner', script, chdir: Rails.root.to_s
    )
    assert status.success?, "boot failed: #{stderr}"
    stdout.strip.split('|')
  end

  # Runs the block with the key provider the app would use for this env and secret.
  # Block-scoped, so nothing leaks past it.
  def with_keys(env, secret:, &block)
    ActiveRecord::Encryption.with_encryption_context(
      key_provider: provider_for(env, secret: secret), &block
    )
  end

  def provider_for(env, secret:)
    derived = ActiveSupport::KeyGenerator
              .new(EncryptionKeys.primary_key(env, secret: secret),
                   hash_digest_class: ActiveRecord::Encryption.config.hash_digest_class)
              .generate_key(EncryptionKeys.key_derivation_salt(env, secret: secret),
                            ActiveRecord::Encryption.cipher.key_length)
    ActiveRecord::Encryption::KeyProvider.new([ActiveRecord::Encryption::Key.new(derived)])
  end

  def raw_value(id)
    AppConfig.connection.select_value("SELECT value FROM app_configs WHERE id = #{id}")
  end
end
