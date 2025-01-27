class ApiKey < ApplicationRecord
  belongs_to :exchange
  belongs_to :user

  attr_encrypted :key, key: ENV.fetch('API_KEY_ENCRRYPTION_KEY')
  attr_encrypted :secret, key: ENV.fetch('API_SECRET_ENCRRYPTION_KEY')
  attr_encrypted :passphrase, key: ENV.fetch('API_PASSPHRASE_ENCRRYPTION_KEY')

  STATES = %i[pending correct incorrect].freeze
  TYPES = %i[trading withdrawal].freeze

  enum status: [*STATES]
  enum key_type: [*TYPES]

  delegate :name, to: :exchange, prefix: true

  scope :for_bot, ->(user_id, exchange_id, key_type = 'trading') {
    where(user_id: user_id, exchange_id: exchange_id, key_type: key_type)
  }
end
