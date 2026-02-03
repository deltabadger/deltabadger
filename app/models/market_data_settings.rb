# frozen_string_literal: true

class MarketDataSettings
  PROVIDER_COINGECKO = 'coingecko'
  PROVIDER_DELTABADGER = 'deltabadger'

  def self.current_provider
    AppConfig.market_data_provider
  end

  def self.configured?
    current_provider.present?
  end

  def self.coingecko?
    current_provider == PROVIDER_COINGECKO
  end

  def self.deltabadger?
    current_provider == PROVIDER_DELTABADGER
  end

  def self.deltabadger_url
    AppConfig.market_data_url
  end

  def self.deltabadger_token
    AppConfig.market_data_token
  end

  def self.deltabadger_available?
    ENV['MARKET_DATA_URL'].present?
  end
end
