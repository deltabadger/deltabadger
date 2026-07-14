# frozen_string_literal: true

class MarketDataSettings
  PROVIDER_COINGECKO = 'coingecko'
  PROVIDER_DELTABADGER = 'deltabadger'

  def self.current_provider
    return PROVIDER_DELTABADGER if deltabadger_available?

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

  # Docker-internal network-alias launchpad's hosted deploy passes as MARKET_DATA_URL (see
  # deltabadger-launchpad's config/deploy.yml) — fast for server-to-server calls but never
  # browser-reachable. data-api's own public host (Kamal-proxied, DNS-only/grey-cloud) serves the
  # same instance's static assets (e.g. /logos/*) directly to browsers. Any other configured
  # MARKET_DATA_URL (self-hosted/BYO market data providers) is already public and used as-is.
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
