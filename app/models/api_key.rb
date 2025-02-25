class ApiKey < ApplicationRecord
  belongs_to :exchange
  belongs_to :user

  attr_encrypted :key, key: ENV.fetch('API_KEY_ENCRRYPTION_KEY')
  attr_encrypted :secret, key: ENV.fetch('API_SECRET_ENCRRYPTION_KEY')
  attr_encrypted :passphrase, key: ENV.fetch('API_PASSPHRASE_ENCRRYPTION_KEY')

  validate :unique_for_user_exchange_and_key_type, on: :create
  validate :validate_key_permissions, if: -> { key.present? && secret.present? }

  STATES = %i[pending correct incorrect].freeze
  TYPES = %i[trading withdrawal].freeze

  enum status: [*STATES]
  enum key_type: [*TYPES]

  delegate :name, to: :exchange, prefix: true

  scope :for_bot, lambda { |user_id, exchange_id, key_type = 'trading'|
    where(user_id: user_id, exchange_id: exchange_id, key_type: key_type)
  }

  private

  def unique_for_user_exchange_and_key_type
    return unless ApiKey.exists?(user_id: user_id, exchange_id: exchange_id, key_type: key_type)

    errors.add(:key, 'An API key for this user, exchange and key type already exists')
  end

  def validate_key_permissions
    result = exchange.check_valid_api_key?(api_key: self)
    if result.success?
      self.status = result.data ? :correct : :incorrect
      if result.data == false
        message = 'Incorrect API key permissions'
        errors.add(:key, message)
        errors.add(:secret, message)
      end
    else
      self.status = :pending
      message = 'Failed to validate API key permissions'
      errors.add(:key, message)
      errors.add(:secret, message)
    end
  end
end
