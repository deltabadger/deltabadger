class EncryptOtpSecretKey < ActiveRecord::Migration[8.1]
  # Encrypts the plaintext otp_secret_key column using attr_encrypted.
  # attr_encrypted with ActiveRecord stores values as base64.

  ALGORITHM = "aes-256-gcm".freeze

  def up
    key = Digest::SHA256.digest(ENV.fetch("APP_ENCRYPTION_KEY"))

    add_column :users, :encrypted_otp_secret_key, :string
    add_column :users, :encrypted_otp_secret_key_iv, :string

    rows = connection.select_all("SELECT id, otp_secret_key FROM users WHERE otp_secret_key IS NOT NULL")

    rows.each do |row|
      plaintext = row["otp_secret_key"]
      next if plaintext.blank?

      iv = OpenSSL::Cipher.new(ALGORITHM).tap(&:encrypt).random_iv
      encrypted = Encryptor.encrypt(
        value: plaintext,
        key: key,
        iv: iv,
        algorithm: ALGORITHM
      )

      encrypted_b64 = [encrypted].pack("m")
      iv_b64 = [iv].pack("m")

      connection.raw_connection.execute(
        "UPDATE users SET encrypted_otp_secret_key = ?, encrypted_otp_secret_key_iv = ? WHERE id = ?",
        [encrypted_b64, iv_b64, row["id"]]
      )
    end

    remove_column :users, :otp_secret_key
  end

  def down
    key = Digest::SHA256.digest(ENV.fetch("APP_ENCRYPTION_KEY"))

    add_column :users, :otp_secret_key, :string

    rows = connection.select_all(
      "SELECT id, encrypted_otp_secret_key, encrypted_otp_secret_key_iv FROM users WHERE encrypted_otp_secret_key IS NOT NULL"
    )

    rows.each do |row|
      encrypted_b64 = row["encrypted_otp_secret_key"]
      iv_b64 = row["encrypted_otp_secret_key_iv"]
      next if encrypted_b64.nil? || iv_b64.nil?

      encrypted_value = encrypted_b64.unpack1("m")
      iv = iv_b64.unpack1("m")
      plaintext = Encryptor.decrypt(
        value: encrypted_value,
        key: key,
        iv: iv,
        algorithm: ALGORITHM
      )

      connection.raw_connection.execute(
        "UPDATE users SET otp_secret_key = ? WHERE id = ?",
        [plaintext, row["id"]]
      )
    end

    remove_column :users, :encrypted_otp_secret_key
    remove_column :users, :encrypted_otp_secret_key_iv
  end
end
