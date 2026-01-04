class FeeApiKey < ApplicationRecord
  belongs_to :exchange

  attr_encrypted :key, key: ENV.fetch('APP_ENCRYPTION_KEY')
  attr_encrypted :secret, key: ENV.fetch('APP_ENCRYPTION_KEY')
  attr_encrypted :passphrase, key: ENV.fetch('APP_ENCRYPTION_KEY')

  delegate :name, to: :exchange, prefix: true
end
