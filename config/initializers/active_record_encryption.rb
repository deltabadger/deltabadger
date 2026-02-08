Rails.application.configure do
  secret = Rails.application.secret_key_base

  config.active_record.encryption.primary_key = Digest::SHA256.hexdigest("#{secret}-ar-encryption-primary")
  config.active_record.encryption.key_derivation_salt = Digest::SHA256.hexdigest("#{secret}-ar-encryption-salt")

  # Allow reading data that was not yet encrypted (for migration transition)
  config.active_record.encryption.support_unencrypted_data = true
end
