class ApiKey < ApplicationRecord
  belongs_to :exchange
  belongs_to :user

  encrypts :key
  encrypts :secret
  encrypts :passphrase

  validate :unique_for_user_exchange_and_key_type, on: :create
  validate :hyperliquid_key_format, if: -> { exchange&.is_a?(Exchanges::Hyperliquid) }

  enum :status, %i[pending_validation correct incorrect]
  enum :key_type, %i[trading withdrawal]

  scope :for_bot, lambda { |user_id, exchange_id, key_type = 'trading'|
    where(user_id: user_id, exchange_id: exchange_id, key_type: key_type)
  }

  def get_validity
    exchange.get_api_key_validity(api_key: self)
  end

  def update_status!(result)
    if result.success?
      if result.data
        update!(status: :correct)
      else
        Rails.logger.warn("[#{exchange.name}] API key validation: incorrect key")
        update!(status: :incorrect)
      end
    else
      Rails.logger.warn("[#{exchange.name}] API key validation failed: #{result.errors.join(', ')}")
      update!(status: :pending_validation)
    end
  end

  private

  def unique_for_user_exchange_and_key_type
    return unless ApiKey.exists?(user_id: user_id, exchange_id: exchange_id, key_type: key_type)

    errors.add(:key, I18n.t('errors.api_key_already_exists', exchange_name: exchange.name))
  end

  def hyperliquid_key_format
    if key.blank? || !key.match?(/\A0x[0-9a-fA-F]{40}\z/)
      errors.add(:key, 'must be a valid Ethereum wallet address (0x followed by 40 hex characters)')
    end

    return if secret.present? && secret.match?(/\A(0x)?[0-9a-fA-F]{64}\z/)

    errors.add(:secret, 'must be a valid agent private key (64 hex characters, optionally prefixed with 0x)')
  end
end
