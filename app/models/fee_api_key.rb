class FeeApiKey < ApplicationRecord
  belongs_to :exchange

  attr_encrypted :key, key: ENV.fetch('DATABASE_API_KEY_ENCRYPTION_KEY')
  attr_encrypted :secret, key: ENV.fetch('DATABASE_API_SECRET_ENCRYPTION_KEY')
  attr_encrypted :passphrase, key: ENV.fetch('DATABASE_API_PASSPHRASE_ENCRYPTION_KEY')

  delegate :name, to: :exchange, prefix: true
end
