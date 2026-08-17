# frozen_string_literal: true

class MarketDataSettings
  PROVIDER_COINGECKO = 'coingecko'
  PROVIDER_DELTABADGER = 'deltabadger'

  def self.current_provider
    return PROVIDER_DELTABADGER if deltabadger_available?

    AppConfig.market_data_provider
  end

  # A database can name the deltabadger provider with no URL or token behind it — a database
  # copied from an install that had credentials to one that does not. Calling that configured
  # sends every caller into a Result::Failure instead of offering the CoinGecko setup step.
  #
  # Only when the provider comes from the database. MARKET_DATA_URL in the environment is a
  # deliberate choice by whoever started the process, and second-guessing it here would send an
  # install with a URL but no token back through the setup wizard.
  def self.configured?
    return deltabadger_url.present? && deltabadger_token.present? if db_selected_deltabadger?

    current_provider.present?
  end

  def self.db_selected_deltabadger?
    !deltabadger_available? && current_provider == PROVIDER_DELTABADGER
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

  def self.deltabadger_credentials_available?
    deltabadger_available? ||
      (AppConfig.platform_connected? && deltabadger_url.present? && deltabadger_token.present?)
  end

  # MARKET_DATA_URL may be a Docker network alias, which is reachable from this process but not
  # from a browser. Asset URLs handed to the browser (e.g. /logos/*) are rewritten to the public
  # host instead. Any other configured URL is already public and used as-is.
  DELTABADGER_DOCKER_HOST = 'data-api'
  DELTABADGER_PUBLIC_URL = 'https://data.deltabadger.com'

  def self.deltabadger_public_url
    url = deltabadger_url
    return url if url.blank?

    URI(url).host == DELTABADGER_DOCKER_HOST ? DELTABADGER_PUBLIC_URL : url
  rescue URI::InvalidURIError
    url
  end
end
