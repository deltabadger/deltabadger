class MigrateToRailsEncryption < ActiveRecord::Migration[8.1]
  # Migrates from attr_encrypted (AES-256-GCM with base64-encoded values)
  # to Rails ActiveRecord Encryption.
  #
  # For api_keys, fee_api_keys, app_configs:
  #   1. Add new plaintext-named columns
  #   2. Decrypt old attr_encrypted data using pure OpenSSL
  #   3. Re-encrypt with Rails AR Encryption and write to new columns
  #   4. Drop old encrypted_* columns
  #
  # For users:
  #   otp_secret_key is already a plaintext column — Rails `encrypts` with
  #   `support_unencrypted_data = true` handles reading it. We encrypt in-place.

  ALGORITHM = "aes-256-gcm".freeze

  def up
    migrate_api_keys
    migrate_fee_api_keys
    migrate_app_configs
    encrypt_user_otp_secrets
  end

  def down
    raise ActiveRecord::IrreversibleMigration,
      "Cannot reverse: would need APP_ENCRYPTION_KEY to re-encrypt with attr_encrypted"
  end

  private

  def migrate_api_keys
    add_column :api_keys, :key, :string
    add_column :api_keys, :secret, :string
    add_column :api_keys, :passphrase, :string

    decrypt_and_reencrypt(:api_keys, %w[key secret passphrase])

    remove_column :api_keys, :encrypted_key
    remove_column :api_keys, :encrypted_key_iv
    remove_column :api_keys, :encrypted_secret
    remove_column :api_keys, :encrypted_secret_iv
    remove_column :api_keys, :encrypted_passphrase
    remove_column :api_keys, :encrypted_passphrase_iv
  end

  def migrate_fee_api_keys
    add_column :fee_api_keys, :key, :string
    add_column :fee_api_keys, :secret, :string
    add_column :fee_api_keys, :passphrase, :string

    decrypt_and_reencrypt(:fee_api_keys, %w[key secret passphrase])

    remove_column :fee_api_keys, :encrypted_key
    remove_column :fee_api_keys, :encrypted_key_iv
    remove_column :fee_api_keys, :encrypted_secret
    remove_column :fee_api_keys, :encrypted_secret_iv
    remove_column :fee_api_keys, :encrypted_passphrase
    remove_column :fee_api_keys, :encrypted_passphrase_iv
  end

  def migrate_app_configs
    add_column :app_configs, :value, :text

    decrypt_and_reencrypt(:app_configs, %w[value])

    remove_column :app_configs, :encrypted_value
    remove_column :app_configs, :encrypted_value_iv
  end

  def encrypt_user_otp_secrets
    encryptor = ActiveRecord::Encryption::Encryptor.new

    rows = connection.select_all(
      "SELECT id, otp_secret_key FROM users WHERE otp_secret_key IS NOT NULL AND otp_secret_key != ''"
    )

    rows.each do |row|
      plaintext = row["otp_secret_key"]
      next if plaintext.blank?

      encrypted = encryptor.encrypt(plaintext)
      connection.execute(
        sanitize("UPDATE users SET otp_secret_key = ? WHERE id = ?", [encrypted, row["id"]])
      )
    end
  end

  def decrypt_and_reencrypt(table, columns)
    old_key = resolve_old_key
    return if old_key.nil? # No key available and no data to migrate — skip

    encryptor = ActiveRecord::Encryption::Encryptor.new

    select_cols = (["id"] + columns.flat_map { |c| ["encrypted_#{c}", "encrypted_#{c}_iv"] }).join(", ")
    rows = connection.select_all("SELECT #{select_cols} FROM #{table}")

    has_encrypted_data = rows.any? { |row| columns.any? { |c| row["encrypted_#{c}"].present? } }

    if has_encrypted_data && old_key == :missing
      raise "Encrypted data exists in #{table} but APP_ENCRYPTION_KEY is not set. " \
            "Set APP_ENCRYPTION_KEY to the value used when the data was originally encrypted."
    end

    return unless has_encrypted_data

    rows.each do |row|
      updates = {}

      columns.each do |col|
        encrypted_b64 = row["encrypted_#{col}"]
        iv_b64 = row["encrypted_#{col}_iv"]

        next if encrypted_b64.blank? || iv_b64.blank?

        plaintext = decrypt_attr_encrypted(encrypted_b64, iv_b64, old_key) # old_key is a hash with :derived and :raw
        updates[col] = encryptor.encrypt(plaintext)
      end

      next if updates.empty?

      set_clause = updates.keys.map { |k| "#{k} = ?" }.join(", ")
      sql = sanitize("UPDATE #{table} SET #{set_clause} WHERE id = ?", [*updates.values, row["id"]])
      connection.execute(sql)
    end
  end

  def resolve_old_key
    raw_key = ENV["APP_ENCRYPTION_KEY"]
    return :missing if raw_key.blank?

    # Return both derived (SHA-256) and raw keys.
    # Production data uses derived key (after ReEncryptWithDerivedKey migration).
    # Dev data after rollback uses the raw key.
    { derived: Digest::SHA256.digest(raw_key), raw: raw_key }
  end

  def decrypt_attr_encrypted(encrypted_b64, iv_b64, keys)
    raw = Base64.decode64(encrypted_b64)
    iv = Base64.decode64(iv_b64)

    # Try derived key first (production), then raw key (dev after rollback)
    [keys[:derived], keys[:raw]].each do |key|
      next if key.bytesize != 32

      begin
        cipher = OpenSSL::Cipher.new(ALGORITHM)
        cipher.decrypt
        cipher.key = key
        cipher.iv = iv
        cipher.auth_tag = raw[-16..]
        cipher.auth_data = ""
        return cipher.update(raw[0..-17]) + cipher.final
      rescue OpenSSL::Cipher::CipherError
        next
      end
    end

    raise "Failed to decrypt with any available key"
  end

  def sanitize(sql, binds)
    binds.each do |bind|
      sql = sql.sub("?", connection.quote(bind))
    end
    sql
  end
end
