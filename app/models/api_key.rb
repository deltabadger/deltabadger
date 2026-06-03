class ApiKey < ApplicationRecord
  belongs_to :exchange
  belongs_to :user
  has_many :account_transactions, dependent: :nullify

  encrypts :key
  encrypts :secret
  encrypts :passphrase
  # IBKR first-party OAuth 1.0a credentials. The RSA private keys + DH param are the crown
  # jewels — never store them plaintext (they'd land in SQLite and the nightly Borg backups).
  encrypts :access_token
  encrypts :rsa_signature_key
  encrypts :rsa_encryption_key
  encrypts :dh_param

  validate :unique_for_user_exchange_and_key_type, on: :create
  validate :hyperliquid_key_format, if: -> { exchange&.is_a?(Exchanges::Hyperliquid) }

  # :pending_activation is IBKR-specific — the consumer key is registered but IBKR hasn't
  # activated it yet (24h–2wk). Appended last so existing integer values are unchanged.
  enum :status, %i[pending_validation correct incorrect pending_activation]
  enum :key_type, %i[trading withdrawal]

  scope :for_bot, lambda { |user_id, exchange_id, key_type = 'trading'|
    where(user_id: user_id, exchange_id: exchange_id, key_type: key_type)
  }

  def get_validity
    exchange.get_api_key_validity(api_key: self)
  end

  def validate_credentials!(params)
    assign_credentials(params)
    result = get_validity
    if result.success? && result.data == :pending_activation
      # IBKR: keys registered, awaiting IBKR activation — persist so the parked bot can start later.
      update!(status: :pending_activation)
    elsif result.success? && result.data
      update!(status: :correct)
    elsif result.success?
      self.status = :incorrect
      Rails.logger.warn("[#{exchange.name}] API key validation: incorrect key")
    else
      self.status = :pending_validation
      Rails.logger.warn("[#{exchange.name}] API key validation failed: #{result.errors.join(', ')}")
    end
    self
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

  # Assigns whatever credential params were submitted. Crypto exchanges send key/secret/passphrase;
  # the IBKR wizard additionally sends the OAuth fields. Uses indifferent `params[...]` (works for
  # both ActionController::Parameters and plain hashes); absent keys assign nil, which is a no-op
  # for exchanges that don't use them.
  def assign_credentials(params)
    self.key = params[:key]
    self.secret = params[:secret]
    self.passphrase = params[:passphrase]
    self.access_token = params[:access_token]
    self.rsa_signature_key = params[:rsa_signature_key]
    self.rsa_encryption_key = params[:rsa_encryption_key]
    self.dh_param = params[:dh_param]
    self.ibkr_realm = params[:ibkr_realm]
  end

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
