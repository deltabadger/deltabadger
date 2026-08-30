class ApiKey < ApplicationRecord
  SYNC_ERROR_LIMIT = 200

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
  # A CAPABILITY, not a category. `trading` and `read_only` sit on one axis and trading is the
  # superset — a key that may trade may already read, which is why the tracker never has to convert
  # anything and a bot key satisfies it as it stands. `withdrawal` is a scope of its own, not a
  # bigger `trading`: every venue's check requires trading to be OFF on one. Appended last so the
  # stored integers of the first two are unchanged.
  enum :key_type, %i[trading withdrawal read_only]

  # Fields assign_credentials will touch, checked against both `ActionController::Parameters`
  # (string-keyed once permitted) and symbol-keyed hashes from direct/test callers.
  CREDENTIAL_FIELDS = %i[
    key secret passphrase access_token
    rsa_signature_key rsa_encryption_key dh_param ibkr_realm
  ].freeze

  scope :for_bot, lambda { |user_id, exchange_id, key_type = 'trading'|
    where(user_id: user_id, exchange_id: exchange_id, key_type: key_type)
  }

  # Every key that can READ, at most one per venue. A subset query, not a list of two types: reading
  # is contained in trading, so a working bot key IS the best reading key that venue has and the
  # fallback needs no rule of its own.
  #
  # One per venue is load-bearing rather than tidy. `AccountTransactionSync#duplicate?` scopes rows
  # the venue gives no id for to the api_key on purpose — so two sub-accounts on one venue do not
  # swallow each other's identical rows — and syncing two keys of the SAME account would therefore
  # import every id-less row twice.
  #
  # ponytail: grouped in Ruby. One user holds a handful of keys, so the row count is the reason —
  # upgrade to a NOT EXISTS correlated subquery only if this ever runs over a fleet-wide scope.
  def self.reading(scope = all)
    scope.correct.where.not(key_type: :withdrawal)
         .group_by { |api_key| [api_key.user_id, api_key.exchange_id] }
         .values.map { |keys| keys.find(&:trading?) || keys.first }
  end

  # A key is validated against the capability it CLAIMS. Trade and withdrawal permission have to be
  # read off each venue's own permission report, because a successful read proves neither; reading
  # needs no such indirection, so it has one shared implementation and no per-venue code.
  def get_validity
    return exchange.get_read_api_key_validity(api_key: self) if read_only?

    exchange.get_api_key_validity(api_key: self)
  end

  # `update_column` is load-bearing, not a shortcut: `activation_stalled?` anchors on `updated_at`,
  # and a failed sync must not look like the user touching their IBKR activation. Do not "clean this
  # up" to `update!`.
  def record_sync_error!(error)
    text = error.is_a?(Exception) ? "#{error.class}: #{error.message}" : error.to_s

    update_column(:last_sync_error, sanitize_sync_error(text))
  end

  # A report that silently omits an exchange is the worst outcome for a tax document, so the
  # report asks every trading key whether its data can be trusted before it renders a single row.
  def sync_issue
    # Whether this key READS, not whether it trades. `trading?` meant "not the withdrawal key" while
    # those were the only two kinds; adding a reading key made it mean something narrower without
    # anyone saying so, and a reading key that could not sync would have reported nothing at all.
    return nil if withdrawal?
    return { exchange: exchange.name, reason: :failed } if last_sync_error.present?
    return { exchange: exchange.name, reason: :never_synced } if correct? && last_synced_at.nil?

    # Data-derived watermarks can lag for quiet, healthy accounts, so their age is deliberately ignored.
    nil
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
      # `last_sync_error` is cleared on both success paths: it describes credentials that have just
      # been replaced, and nothing else clears it until a sync succeeds. Left behind it keeps
      # `sync_issue` bannering the tax report for an exchange that is fine — and since a sync only
      # runs when the user opens the tracker, "until the next sync" can be months.
      update!(status: :pending_activation, updated_at: Time.current, last_sync_error: nil)
    elsif result.success? && result.data
      update!(status: :correct, last_sync_error: nil)
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

    user.bots.working.each do |bot|
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

  # Credentials and PII must never land in this user-visible-adjacent diagnostics column.
  def sanitize_sync_error(text)
    text
      .gsub(/[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/i, '[redacted]')
      .gsub(%r{(https?://\S+?)\?\S*}, '\\1?[redacted]')
      .gsub(/(?<![A-Za-z0-9_-])(?=[A-Za-z0-9_-]{20,}(?![A-Za-z0-9_-]))(?=[A-Za-z0-9_-]*\d)[A-Za-z0-9_-]+/,
            '[redacted]')
      .gsub(/\d{9,}/, '[redacted]')[0, SYNC_ERROR_LIMIT]
  end

  # Assigns only the credential fields actually present in the submitted params. The generic
  # add-api-key forms send key/secret alone; assigning the whole set would NULL the IBKR RSA
  # material and DH param, which IBKR only lets a user register once. An explicitly-supplied
  # nil still clears the field. Accepts either a symbol or string key for each field so a
  # string-keyed hash doesn't silently skip every assignment and leave get_validity checking
  # stale, previously-stored credentials instead of what was just submitted.
  def assign_credentials(params)
    CREDENTIAL_FIELDS.each do |field|
      submitted_key = [field, field.to_s].find { |name| params.key?(name) }
      self[field] = params[submitted_key] if submitted_key
    end
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
