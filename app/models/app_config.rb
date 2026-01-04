class AppConfig < ApplicationRecord
  validates :key, presence: true, uniqueness: true

  COINGECKO_API_KEY = 'coingecko_api_key'.freeze
  SETUP_SYNC_STATUS = 'setup_sync_status'.freeze

  SYNC_STATUS_PENDING = 'pending'.freeze
  SYNC_STATUS_IN_PROGRESS = 'in_progress'.freeze
  SYNC_STATUS_COMPLETED = 'completed'.freeze

  def self.get(key)
    find_by(key: key)&.value
  end

  def self.set(key, value)
    config = find_or_initialize_by(key: key)
    config.value = value
    config.save!
    config
  end

  def self.coingecko_api_key
    get(COINGECKO_API_KEY) || ENV.fetch('COINGECKO_API_KEY', '')
  end

  def self.coingecko_api_key=(value)
    set(COINGECKO_API_KEY, value)
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
end
