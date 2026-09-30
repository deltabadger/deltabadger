require 'test_helper'

# The Rust crate (rust/) shares this app's database. These are values it wrote, recorded by
# `cargo run --bin record_rust_vectors`. If this fails, an install switched back from the Rust backend
# would not be able to read its own credentials or log in.
class RustVectorsTest < ActiveSupport::TestCase
  VECTORS = JSON.parse(Rails.root.join('rust/tests/fixtures/rust_vectors.json').read)

  # Explicit key bytes, so no global encryption config is touched (parallel workers share it).
  def provider
    secret = VECTORS.fetch('secret_key_base')
    bytes = ActiveSupport::KeyGenerator.new(EncryptionKeys.derived_primary_key(secret),
                                            hash_digest_class: OpenSSL::Digest::SHA256, iterations: 2**16)
                                       .generate_key(EncryptionKeys.derived_salt(secret), 32)
    ActiveRecord::Encryption::KeyProvider.new(ActiveRecord::Encryption::Key.new(bytes))
  end

  test 'the encryption settings are the ones the Rust crate implements' do
    config = ActiveRecord::Encryption.config
    assert_equal OpenSSL::Digest::SHA256, config.hash_digest_class
    assert config.support_unencrypted_data
    assert_not config.store_key_references
  end

  test 'rails decrypts every value the rust crate encrypted' do
    VECTORS.fetch('ciphertexts').each do |name, c|
      assert_not_equal c['plain'], c['cipher'], name
      plain = ActiveRecord::Encryption.with_encryption_context(key_provider: provider) do
        ApiKey.type_for_attribute(:secret).deserialize(c['cipher'])
      end
      assert_equal c['plain'], plain, name
    end
  end

  test 'devise verifies every password hash the rust crate wrote' do
    VECTORS.fetch('bcrypt').each do |b|
      assert Devise::Encryptor.compare(User, b['hash'], b['password']), b['password'][0, 20]
      assert_not Devise::Encryptor.compare(User, b['hash'], 'wrong')
    end
  end
end
