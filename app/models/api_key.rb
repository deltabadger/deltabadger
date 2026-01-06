class ApiKey < ApplicationRecord
  belongs_to :exchange
  belongs_to :user

  attr_encrypted :key, key: ENV.fetch('APP_ENCRYPTION_KEY')
  attr_encrypted :secret, key: ENV.fetch('APP_ENCRYPTION_KEY')
  attr_encrypted :passphrase, key: ENV.fetch('APP_ENCRYPTION_KEY')

  validate :unique_for_user_exchange_and_key_type, on: :create

  enum status: %i[pending_validation correct incorrect]
  enum key_type: %i[trading]

  scope :for_bot, lambda { |user_id, exchange_id, key_type = 'trading'|
    where(user_id: user_id, exchange_id: exchange_id, key_type: key_type)
  }

  def get_validity
    exchange.get_api_key_validity(api_key: self)
  end

  def update_status!(result)
    if result.success?
      update!(status: result.data ? :correct : :incorrect)
    else
      update!(status: :pending_validation)
    end
  end

  private

  def unique_for_user_exchange_and_key_type
    return unless ApiKey.exists?(user_id: user_id, exchange_id: exchange_id, key_type: key_type)

    errors.add(:key, I18n.t('errors.api_key_already_exists', exchange_name: exchange.name))
  end
end
