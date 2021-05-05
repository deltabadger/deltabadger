class ApiKey < ApplicationRecord
  belongs_to :exchange
  belongs_to :user

  attr_encrypted :key, key: ENV.fetch('API_KEY_ENCRRYPTION_KEY')
  attr_encrypted :secret, key: ENV.fetch('API_SECRET_ENCRRYPTION_KEY')
  attr_encrypted :passphrase, key: ENV.fetch('API_PASSPHRASE_ENCRRYPTION_KEY')

  STATES = %i[pending correct incorrect].freeze

  enum status: [*STATES]

  delegate :name, to: :exchange, prefix: true
end
