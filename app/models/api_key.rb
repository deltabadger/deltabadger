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
  # The last line of defence for a retired venue: the wizards, the tracker and legacy
  # POST /api/api_keys all reach key creation by exchange id, so blocking it here covers every door
  # at once. Bot#api_key only BUILDS its fallback key, so an unsaved record still renders fine.
  validate :exchange_not_retired

  # :pending_activation is IBKR-specific — the consumer key is registered but IBKR hasn't
  # activated it yet (24h–2wk). Appended last so existing integer values are unchanged.
  enum :status, %i[pending_validation correct incorrect pending_activation]

  # IBKR activates a self-service consumer key on its weekend server restart. A registration still
  # pending after this long has sat through several of those restarts and is never going to
  # activate — it is invalid, or it was replaced by a later save in IBKR's portal (which keeps only
  # one registration per login). Past this point the wizard stops promising activation and offers a
  # way out instead.
  ACTIVATION_DEADLINE = 14.days
  enum :key_type, %i[trading withdrawal]

  scope :for_bot, lambda { |user_id, exchange_id, key_type = 'trading'|
    where(user_id: user_id, exchange_id: exchange_id, key_type: key_type)
  }

  def get_validity
    exchange.get_api_key_validity(api_key: self)
  end

  # Anchored on updated_at, which on a key awaiting IBKR activation moves only when the user
  # submits credentials: Ibkr::CheckActivationJob writes only on success, and the nightly balance
  # and transaction syncs both scope to :correct, so nothing else touches the row.
  def activation_stalled?
    pending_activation? && updated_at <= ACTIVATION_DEADLINE.ago
  end

  def validate_credentials!(params)
    assign_credentials(params)
    # A retired venue has no API left to ask; short-circuit before calling the stub so the caller
    # sees the retirement rather than a generic "we couldn't verify your key".
    if exchange&.retired?
      self.status = :incorrect
      return self
    end

    result = get_validity
    if result.success? && result.data == :pending_activation
      # IBKR: keys registered, awaiting IBKR activation — persist so the parked bot can start later.
      # updated_at is bumped explicitly even when nothing else changed: activation_stalled? reads it
      # as "when the user last submitted credentials", and resubmitting IDENTICAL credentials — the
      # real fix for a registration redone on a working portal host — dirties nothing, so Active
      # Record would issue no UPDATE and the already-expired clock would survive the retry.
      update!(status: :pending_activation, updated_at: Time.current)
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

  # Stop the owner's still-working bots that trade on this key's exchange, before the key is
  # deleted — otherwise they'd keep firing with no credential. Mirrors SettingsController's
  # stop_working_bots so every key-deletion path leaves bots cleanly stopped.
  def stop_dependent_bots!
    return unless trading?

    user.bots.not_deleted.not_stopped.each do |bot|
      bot.stop if bot.exchange_id == exchange_id
    end
  end

  def update_status!(result)
    if result.success?
      case result.data
      when :pending_activation
        # This runs on a passive GET re-poll (the wizard/tracker/withdrawal "new" actions check
        # the key's validity on every page view), not a credential submission — do not bump
        # updated_at here, or activation_stalled? could never fire: a user revisiting the page
        # would keep restarting its own deadline. Marking :correct here would also defeat
        # Bot::ActionJob's guard against trading on a key IBKR has not activated yet.
        update!(status: :pending_activation)
      when nil, false
        Rails.logger.warn("[#{exchange.name}] API key validation: incorrect key")
        update!(status: :incorrect)
      else
        update!(status: :correct)
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

  def exchange_not_retired
    return unless exchange&.retired?

    errors.add(:base, I18n.t('errors.exchange_retired'))
  end

  def hyperliquid_key_format
    if key.blank? || !key.match?(/\A0x[0-9a-fA-F]{40}\z/)
      errors.add(:key, 'must be a valid Ethereum wallet address (0x followed by 40 hex characters)')
    end

    return if secret.present? && secret.match?(/\A(0x)?[0-9a-fA-F]{64}\z/)

    errors.add(:secret, 'must be a valid agent private key (64 hex characters, optionally prefixed with 0x)')
  end
end
