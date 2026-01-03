class AppConfig < ApplicationRecord
  validates :key, presence: true, uniqueness: true

  COINGECKO_API_KEY = 'coingecko_api_key'.freeze

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
end
