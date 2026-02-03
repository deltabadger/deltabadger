class AppConfig < ApplicationRecord
  validates :key, presence: true, uniqueness: true

  attr_encrypted :value, key: ENV.fetch('APP_ENCRYPTION_KEY')

  COINGECKO_API_KEY = 'coingecko_api_key'.freeze
  SETUP_SYNC_STATUS = 'setup_sync_status'.freeze

  SYNC_STATUS_PENDING = 'pending'.freeze
  SYNC_STATUS_IN_PROGRESS = 'in_progress'.freeze
  SYNC_STATUS_COMPLETED = 'completed'.freeze

  # Registration
  REGISTRATION_OPEN = 'registration_open'.freeze

  # SMTP/Email notification settings
  SMTP_PROVIDER = 'smtp_provider'.freeze         # 'gmail_smtp' or 'env_smtp'
  SMTP_GMAIL_EMAIL = 'smtp_gmail_email'.freeze
  SMTP_GMAIL_PASSWORD = 'smtp_gmail_password'.freeze

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

  def self.smtp_gmail_email
    get(SMTP_GMAIL_EMAIL)
  end

  def self.smtp_gmail_email=(value)
    set(SMTP_GMAIL_EMAIL, value)
  end

  def self.smtp_gmail_password
    get(SMTP_GMAIL_PASSWORD)
  end

  def self.smtp_gmail_password=(value)
    set(SMTP_GMAIL_PASSWORD, value)
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

  def self.clear_smtp_settings!
    delete(SMTP_PROVIDER)
    delete(SMTP_GMAIL_EMAIL)
    delete(SMTP_GMAIL_PASSWORD)
  end
end
