class AppConfig < ApplicationRecord
  validates :key, presence: true, uniqueness: true

  encrypts :value

  COINGECKO_API_KEY = 'coingecko_api_key'.freeze
  SETUP_SYNC_STATUS = 'setup_sync_status'.freeze

  # Market data provider settings
  MARKET_DATA_PROVIDER = 'market_data_provider'.freeze # 'coingecko' or 'deltabadger'
  MARKET_DATA_URL = 'market_data_url'.freeze
  MARKET_DATA_TOKEN = 'market_data_token'.freeze

  SYNC_STATUS_PENDING = 'pending'.freeze
  SYNC_STATUS_IN_PROGRESS = 'in_progress'.freeze
  SYNC_STATUS_COMPLETED = 'completed'.freeze

  # Registration
  REGISTRATION_OPEN = 'registration_open'.freeze

  # SMTP/Email notification settings
  # Alpaca (stocks) settings
  ALPACA_API_KEY = 'alpaca_api_key'.freeze
  ALPACA_API_SECRET = 'alpaca_api_secret'.freeze
  ALPACA_MODE = 'alpaca_mode'.freeze # 'paper' or 'live'

  # MCP (Model Context Protocol) settings
  MCP_TOOL_DEFAULTS = {
    'list_bots' => true,
    'get_bot_details' => true,
    'list_exchanges' => true,
    'get_exchange_balances' => true,
    'get_portfolio_summary' => true,
    'list_transactions' => true,
    'create_bot' => false,
    'start_bot' => false,
    'stop_bot' => false,
    'update_bot_settings' => false,
    'start_rule' => false,
    'stop_rule' => false,
    'update_rule_settings' => false,
    'list_open_orders' => true,
    'market_buy' => false,
    'market_sell' => false,
    'limit_buy' => false,
    'limit_sell' => false,
    'cancel_order' => false
  }.freeze

  MCP_TOOL_GROUPS = {
    'read' => %w[list_bots get_bot_details list_exchanges get_exchange_balances get_portfolio_summary list_transactions list_open_orders],
    'control' => %w[create_bot start_bot stop_bot update_bot_settings start_rule stop_rule update_rule_settings],
    'trade' => %w[market_buy market_sell limit_buy limit_sell cancel_order]
  }.freeze

  SMTP_PROVIDER = 'smtp_provider'.freeze # 'custom_smtp' or 'env_smtp'
  SMTP_USERNAME = 'smtp_username'.freeze
  SMTP_PASSWORD = 'smtp_password'.freeze
  SMTP_HOST = 'smtp_host'.freeze
  SMTP_PORT = 'smtp_port'.freeze

  def self.get(key)
    find_by(key: key)&.value
  end

  def self.set(key, value)
    config = find_or_initialize_by(key: key)
    config.value = value
    config.save!
    config
  end

  def self.delete(key)
    find_by(key: key)&.destroy
  end

  def self.coingecko_api_key
    record = find_by(key: COINGECKO_API_KEY)
    # If record exists in DB, use its value (even if empty - user explicitly cleared it)
    # Only fall back to ENV when no DB record exists (initial setup)
    return record.value if record

    ENV.fetch('COINGECKO_API_KEY', '')
  end

  def self.coingecko_api_key=(value)
    set(COINGECKO_API_KEY, value)
  end

  def self.coingecko_configured?
    coingecko_api_key.present?
  end

  def self.setup_completed?
    User.exists?(admin: true)
  rescue ActiveRecord::StatementInvalid
    false
  end

  def self.setup_sync_status
    get(SETUP_SYNC_STATUS)
  end

  def self.setup_sync_status=(value)
    set(SETUP_SYNC_STATUS, value)
  end

  def self.setup_sync_pending?
    setup_sync_status == SYNC_STATUS_PENDING
  end

  def self.setup_sync_in_progress?
    setup_sync_status == SYNC_STATUS_IN_PROGRESS
  end

  def self.setup_sync_completed?
    setup_sync_status == SYNC_STATUS_COMPLETED
  end

  def self.setup_sync_needed?
    setup_sync_pending? || setup_sync_in_progress?
  end

  # Registration configuration
  def self.registration_open?
    get(REGISTRATION_OPEN) == 'true'
  end

  def self.registration_open=(value)
    set(REGISTRATION_OPEN, value.to_s)
  end

  # SMTP configuration methods
  def self.smtp_provider
    get(SMTP_PROVIDER)
  end

  def self.smtp_provider=(value)
    if value.nil? || value.blank?
      delete(SMTP_PROVIDER)
    else
      set(SMTP_PROVIDER, value)
    end
  end

  def self.smtp_username
    get(SMTP_USERNAME)
  end

  def self.smtp_username=(value)
    set(SMTP_USERNAME, value)
  end

  def self.smtp_password
    get(SMTP_PASSWORD)
  end

  def self.smtp_password=(value)
    set(SMTP_PASSWORD, value)
  end

  def self.smtp_host
    get(SMTP_HOST)
  end

  def self.smtp_host=(value)
    set(SMTP_HOST, value)
  end

  def self.smtp_port
    get(SMTP_PORT)
  end

  def self.smtp_port=(value)
    set(SMTP_PORT, value)
  end

  def self.smtp_configured?
    smtp_provider.present?
  end

  def self.smtp_env_available?
    ENV['SMTP_ADDRESS'].present?
  end

  def self.smtp_env_provider_name
    ENV.fetch('SMTP_PROVIDER_NAME', ENV['SMTP_ADDRESS'])
  end

  def self.notifications_sender
    ENV.fetch('NOTIFICATIONS_SENDER', nil) ||
      smtp_username.presence ||
      'noreply@localhost'
  end

  def self.clear_smtp_settings!
    delete(SMTP_PROVIDER)
    delete(SMTP_USERNAME)
    delete(SMTP_PASSWORD)
    delete(SMTP_HOST)
    delete(SMTP_PORT)
  end

  # Market data provider configuration methods
  def self.market_data_provider
    get(MARKET_DATA_PROVIDER)
  end

  def self.market_data_provider=(value)
    if value.nil? || value.blank?
      delete(MARKET_DATA_PROVIDER)
    else
      set(MARKET_DATA_PROVIDER, value)
    end
  end

  def self.market_data_url
    record = find_by(key: MARKET_DATA_URL)
    return record.value if record

    ENV.fetch('MARKET_DATA_URL', '')
  end

  def self.market_data_url=(value)
    set(MARKET_DATA_URL, value)
  end

  def self.market_data_token
    record = find_by(key: MARKET_DATA_TOKEN)
    return record.value if record

    ENV.fetch('MARKET_DATA_TOKEN', '')
  end

  def self.market_data_token=(value)
    set(MARKET_DATA_TOKEN, value)
  end

  def self.market_data_configured?
    MarketDataSettings.configured?
  end

  def self.market_data_env_available?
    ENV['MARKET_DATA_URL'].present?
  end

  def self.market_data_env_provider_name
    ENV.fetch('MARKET_DATA_PROVIDER_NAME', ENV['MARKET_DATA_URL'])
  end

  # Alpaca (stocks) configuration methods
  def self.alpaca_configured?
    get(ALPACA_API_KEY).present?
  end

  def self.clear_alpaca_settings!
    delete(ALPACA_API_KEY)
    delete(ALPACA_API_SECRET)
    delete(ALPACA_MODE)
  end

  def self.clear_market_data_settings!
    delete(MARKET_DATA_PROVIDER)
    delete(MARKET_DATA_URL)
    delete(MARKET_DATA_TOKEN)
  end

  # MCP configuration methods
  def self.mcp_configured?
    ENV['MCP_ENABLED'] == 'true'
  end

  def self.mcp_url
    base = ENV.fetch('APP_ROOT_URL', 'http://localhost:3000')
    "#{base}/mcp"
  end
end
