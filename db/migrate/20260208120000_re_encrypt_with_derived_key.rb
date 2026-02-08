class ReEncryptWithDerivedKey < ActiveRecord::Migration[8.1]
  # Re-encrypts all attr_encrypted columns from raw APP_ENCRYPTION_KEY
  # to SHA-256 derived key. This ensures any length key works correctly
  # with AES-256-GCM (which requires exactly 32 bytes).
  #
  # attr_encrypted with ActiveRecord stores both the encrypted value and IV
  # as base64-encoded strings (encode: true is the default for AR adapter).

  ALGORITHM = "aes-256-gcm".freeze

  TABLES = {
    api_keys: %w[key secret passphrase],
    fee_api_keys: %w[key secret passphrase],
    app_configs: %w[value]
  }.freeze

  def up
    old_key = ENV.fetch("APP_ENCRYPTION_KEY")
    new_key = Digest::SHA256.digest(old_key)
    re_encrypt_all(old_key, new_key)
  end

  def down
    old_key = ENV.fetch("APP_ENCRYPTION_KEY")
    new_key = Digest::SHA256.digest(old_key)
    re_encrypt_all(new_key, old_key)
  end

  private

  def re_encrypt_all(from_key, to_key)
    TABLES.each do |table, columns|
      re_encrypt_table(table, columns, from_key, to_key)
    end
  end

  def re_encrypt_table(table, columns, from_key, to_key)
    select_cols = columns.flat_map { |c| ["encrypted_#{c}", "encrypted_#{c}_iv"] }.join(", ")
    rows = connection.select_all("SELECT id, #{select_cols} FROM #{table}")

    rows.each do |row|
      updates = {}

      columns.each do |col|
        encrypted_b64 = row["encrypted_#{col}"]
        iv_b64 = row["encrypted_#{col}_iv"]

        next if encrypted_b64.nil? || iv_b64.nil?

        # Decode base64 stored values
        encrypted_value = encrypted_b64.unpack1("m")
        iv = iv_b64.unpack1("m")

        plaintext = Encryptor.decrypt(
          value: encrypted_value,
          key: from_key,
          iv: iv,
          algorithm: ALGORITHM
        )

        # Re-encrypt with new key
        new_iv = OpenSSL::Cipher.new(ALGORITHM).tap(&:encrypt).random_iv
        new_encrypted = Encryptor.encrypt(
          value: plaintext,
          key: to_key,
          iv: new_iv,
          algorithm: ALGORITHM
        )

        # Encode back to base64 for storage
        updates["encrypted_#{col}"] = [new_encrypted].pack("m")
        updates["encrypted_#{col}_iv"] = [new_iv].pack("m")
      end

      next if updates.empty?

      set_clause = updates.keys.map { |k| "#{k} = ?" }.join(", ")
      sql = "UPDATE #{table} SET #{set_clause} WHERE id = ?"
      connection.raw_connection.execute(sql, [*updates.values, row["id"]])
    end
  end
end
