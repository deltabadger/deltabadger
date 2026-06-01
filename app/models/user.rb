class User < ApplicationRecord
  include ActionCable::Channel::Broadcasting

  attr_accessor :otp_code_token

  devise :database_authenticatable, :registerable,
         :recoverable, :rememberable, :validatable, :confirmable, :lockable

  encrypts :otp_secret_key
  has_one_time_password
  enum :otp_module, %i[disabled enabled], prefix: true
  has_many :api_keys
  has_many :account_transactions
  has_many :exchanges, through: :api_keys
  has_many :bots
  has_many :transactions, through: :bots
  has_many :rules, dependent: :destroy
  has_many :oauth_access_tokens, class_name: 'Doorkeeper::AccessToken', foreign_key: :resource_owner_id, dependent: :destroy
  has_many :mcp_applications, -> { where(personal_access_token: [false, nil]).distinct },
           through: :oauth_access_tokens, source: :application, class_name: 'Doorkeeper::Application'
  has_one :personal_api_application,
          -> { where(personal_access_token: true) },
          class_name: 'Doorkeeper::Application', foreign_key: :personal_owner_id, dependent: :destroy

  validates :name, presence: true, if: -> { new_record? }
  validate :validate_name, if: -> { new_record? || name_changed? }
  validate :validate_email, if: -> { new_record? || email_changed? }
  validate :password_complexity, if: -> { password.present? }
  validates :time_zone, inclusion: { in: ActiveSupport::TimeZone.all.map(&:name), allow_nil: true }

  def global_pnl(use_cache: true)
    invested_by_currency = Hash.new(0)
    value_by_currency = Hash.new(0)
    has_any_metrics = false

    bots.not_deleted.each do |bot|
      next unless bot.dca_single_asset? || bot.dca_dual_asset? || bot.dca_index? || bot.signal?

      metrics = if use_cache
                  bot.metrics_with_current_prices_from_cache || bot.metrics_with_current_prices
                else
                  bot.metrics_with_current_prices
                end
      next if metrics.nil?

      currency = bot.quote_asset&.symbol
      next if currency.nil?

      has_any_metrics = true
      invested_by_currency[currency] += metrics[:total_quote_amount_invested] || 0
      value_by_currency[currency] += metrics[:total_amount_value_in_quote] || 0
    end

    return nil unless has_any_metrics

    invested_result = Utilities::Currency.batch_convert(invested_by_currency, to: 'USD')
    return nil if invested_result.failure?

    value_result = Utilities::Currency.batch_convert(value_by_currency, to: 'USD')
    return nil if value_result.failure?

    total_invested_usd = invested_result.data
    total_value_usd = value_result.data

    return nil if total_invested_usd.zero?

    profit_usd = total_value_usd - total_invested_usd
    pnl_percent = profit_usd / total_invested_usd

    {
      percent: pnl_percent,
      profit_usd: profit_usd
    }
  end

  # Cache-only global PnL for the /bots index — never makes a live exchange or FX call.
  # Returns { result: { percent:, profit_usd: } | nil, loading: Boolean } with three states:
  #   * ready   -> { result: {...}, loading: false }
  #   * loading -> { result: nil,   loading: true  }   a needed bot-metric/FX cache is cold
  #   * empty   -> { result: nil,   loading: false }   nothing invested; render nothing, no spinner
  # Completeness only considers bots that have submitted transactions, so a user whose
  # only bots are fresh/empty never gets a perpetual spinner.
  def global_pnl_snapshot(cache_only: true)
    invested_by_currency = Hash.new(0)
    value_by_currency = Hash.new(0)
    loading = false

    bots.not_deleted.each do |bot|
      next unless bot.dca_single_asset? || bot.dca_dual_asset? || bot.dca_index? || bot.signal?

      metrics = bot.metrics_with_current_prices_from_cache
      if metrics.nil?
        # A bot that has actually traded is expected to have a cache entry shortly (the
        # warm job / per-bot broadcast fills it). A bot with no submitted transactions
        # contributes nothing and must not hold the dashboard in a spinner.
        loading = true if bot.transactions.submitted.exists?
        next
      end

      currency = bot.quote_asset&.symbol
      next if currency.nil?

      invested_by_currency[currency] += metrics[:total_quote_amount_invested] || 0
      value_by_currency[currency] += metrics[:total_amount_value_in_quote] || 0
    end

    invested_result = Utilities::Currency.batch_convert(invested_by_currency, to: 'USD', cache_only: cache_only)
    value_result = Utilities::Currency.batch_convert(value_by_currency, to: 'USD', cache_only: cache_only)
    loading ||= invested_result.failure? || value_result.failure?

    # Never expose a partial total — the global-pnl partial renders any present value.
    return { result: nil, loading: true } if loading

    total_invested_usd = invested_result.data
    return { result: nil, loading: false } if total_invested_usd.zero?

    profit_usd = value_result.data - total_invested_usd
    { result: { percent: profit_usd / total_invested_usd, profit_usd: profit_usd }, loading: false }
  end

  def broadcast_global_pnl_update
    broadcast_replace_to(
      ["user_#{id}", :bot_updates],
      target: 'global-pnl',
      partial: 'bots/global_pnl',
      locals: { global_pnl: global_pnl, loading: false }
    )
  end

  # MCP permissions (per-user)

  def mcp_tool_enabled?(tool_name)
    return false unless AppConfig::MCP_TOOL_DEFAULTS.key?(tool_name)

    overrides = mcp_settings['tool_permissions'] || {}
    return overrides[tool_name] if overrides.key?(tool_name)

    AppConfig::MCP_TOOL_DEFAULTS[tool_name]
  end

  def set_mcp_tool_enabled(tool_name, enabled)
    self.mcp_settings = mcp_settings.merge(
      'tool_permissions' => (mcp_settings['tool_permissions'] || {}).merge(tool_name => enabled)
    )
    save!
  end

  def set_mcp_tool_group_enabled(group, enabled)
    tools = AppConfig::MCP_TOOL_GROUPS[group]
    perms = mcp_settings['tool_permissions'] || {}
    tools.each { |t| perms[t] = enabled }
    self.mcp_settings = mcp_settings.merge('tool_permissions' => perms)
    save!
  end

  def mcp_tool_permissions
    overrides = mcp_settings['tool_permissions'] || {}
    AppConfig::MCP_TOOL_DEFAULTS.merge(overrides)
  end

  def enabled_mcp_tool_names
    mcp_tool_permissions.select { |_, v| v }.keys
  end

  # REST API permissions (per-user). Mirrors MCP; stored in `rest_settings` JSON column.

  def rest_tool_enabled?(tool_name)
    return false unless AppConfig::REST_TOOL_DEFAULTS.key?(tool_name)

    overrides = rest_settings['tool_permissions'] || {}
    return overrides[tool_name] if overrides.key?(tool_name)

    AppConfig::REST_TOOL_DEFAULTS[tool_name]
  end

  def set_rest_tool_enabled(tool_name, enabled)
    self.rest_settings = rest_settings.merge(
      'tool_permissions' => (rest_settings['tool_permissions'] || {}).merge(tool_name => enabled)
    )
    save!
  end

  def set_rest_tool_group_enabled(group, enabled)
    tools = AppConfig::REST_TOOL_GROUPS[group]
    return unless tools

    perms = rest_settings['tool_permissions'] || {}
    tools.each { |t| perms[t] = enabled }
    self.rest_settings = rest_settings.merge('tool_permissions' => perms)
    save!
  end

  def rest_tool_permissions
    overrides = rest_settings['tool_permissions'] || {}
    AppConfig::REST_TOOL_DEFAULTS.merge(overrides)
  end

  def enabled_rest_tool_names
    rest_tool_permissions.select { |_, v| v }.keys
  end

  def mcp_dry_run?
    mcp_settings['dry_run'] == true
  end

  def mcp_dry_run=(value)
    self.mcp_settings = mcp_settings.merge('dry_run' => value ? true : false)
    save!
  end

  # Personal REST API token. One per user, no expiry, scoped to :api only.
  # Lazy-minted on first read; regenerate replaces (revokes old + mints new)
  # atomically under the personal-application row lock.

  def personal_api_token
    ensure_personal_api_token!
  end

  def ensure_personal_api_token!
    app = personal_api_application || create_personal_api_app_safely!
    app.with_lock do
      active_personal_token_for(app) || mint_personal_token!(app)
    end
  end

  def regenerate_personal_api_token!
    app = personal_api_application || create_personal_api_app_safely!
    app.with_lock do
      Doorkeeper::AccessToken
        .where(application_id: app.id, resource_owner_id: id, revoked_at: nil)
        .update_all(revoked_at: Time.current)
      mint_personal_token!(app)
    end
  end

  private

  # ---- personal API token helpers ----------------------------------------

  # Race-safe app creation. The partial unique index on
  # oauth_applications(personal_owner_id) WHERE personal_access_token = 1
  # guarantees that under concurrent first-renders only one INSERT wins;
  # the loser rescues the unique-violation and re-reads the row.
  def create_personal_api_app_safely!
    create_personal_api_application!(
      name: 'Personal API token',
      redirect_uri: 'https://localhost/personal-access-token',
      confidential: false,
      scopes: 'api',
      personal_access_token: true
    )
  rescue ActiveRecord::RecordNotUnique
    reload
    personal_api_application or raise
  end

  # Explicit query, not Doorkeeper::AccessToken.active_for — that helper's
  # signature varies between Doorkeeper versions.
  def active_personal_token_for(app)
    Doorkeeper::AccessToken
      .where(application_id: app.id, resource_owner_id: id, revoked_at: nil)
      .reject(&:expired?)
      .first
  end

  # LOAD-BEARING: resource_owner_id must always equal user.id. The User#destroy
  # cascade story depends on this — every personal token is caught by the
  # user's own `has_many :oauth_access_tokens, dependent: :destroy`.
  def mint_personal_token!(app)
    Doorkeeper::AccessToken.create!(
      application: app, resource_owner_id: id,
      token: SecureRandom.hex(32), scopes: 'api', expires_in: nil
    )
  end

  # ------------------------------------------------------------------------

  def set_default_time_zone
    self.time_zone = 'UTC' if time_zone.blank?
  end

  def validate_name
    valid_name = name =~ Regexp.new(Name::PATTERN)
    errors.add(:name, I18n.t('devise.registrations.new.name_invalid')) unless valid_name
  end

  def validate_email
    valid_email = email =~ Regexp.new(Email::ADDRESS_PATTERN)
    errors.add(:email, I18n.t('devise.registrations.new.email_invalid')) unless valid_email
    errors.add(:email, :taken) if Email.google_email_exists?(email, exclude_emails: [email_was].compact)
  end

  def password_complexity
    complexity_is_valid = password =~ Regexp.new(Password::PATTERN)
    errors.add(:password, I18n.t('errors.messages.too_simple_password')) unless complexity_is_valid
  end
end
