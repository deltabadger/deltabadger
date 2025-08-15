class FeeApiKey < ApplicationRecord
  belongs_to :exchange

  attr_encrypted :key, key: ENV.fetch('API_KEY_ENCRYPTION_KEY')
  attr_encrypted :secret, key: ENV.fetch('API_SECRET_ENCRYPTION_KEY')
  attr_encrypted :passphrase, key: ENV.fetch('API_PASSPHRASE_ENCRYPTION_KEY')

  delegate :name, to: :exchange, prefix: true
end
