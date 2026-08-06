module EncryptedCredentialsTestHelpers
  # Produces a valid encrypted message this install's key cannot decrypt — what actually
  # sits in the database of an operator who already changed their secret. Built from an
  # explicitly derived key rather than by reconfiguring ActiveRecord::Encryption globally,
  # which would leak across the parallel test workers.
  def ciphertext_under_foreign_key(plaintext)
    derived = ActiveSupport::KeyGenerator
              .new(Digest::SHA256.hexdigest('foreign-primary'), hash_digest_class: OpenSSL::Digest::SHA256)
              .generate_key(Digest::SHA256.hexdigest('foreign-salt'), 32)
    provider = ActiveRecord::Encryption::KeyProvider.new([ActiveRecord::Encryption::Key.new(derived)])
    ActiveRecord::Encryption.encryptor.encrypt(plaintext, key_provider: provider)
  end

  # update_all(column: value) type-casts through the encrypted attribute type, so passing a
  # foreign blob there encrypts it AGAIN under the current key. Reading it back then peels
  # one layer and returns the blob — the fixture would look correct while testing double
  # encryption rather than a wrong-key read. Measured: the column came back 190 bytes for a
  # 90-byte blob. Writing the column directly is the only way to reproduce what is actually
  # on a stranded operator's disk.
  def write_raw(model, id, column, value)
    connection = model.connection
    connection.execute(
      "UPDATE #{model.quoted_table_name} SET #{connection.quote_column_name(column)} = " \
      "#{connection.quote(value)} WHERE id = #{connection.quote(id)}"
    )
  end
end
