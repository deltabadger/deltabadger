# frozen_string_literal: true

class MarketDataSettings
  PROVIDER_COINGECKO = 'coingecko'
  PROVIDER_DELTABADGER = 'deltabadger'

  def self.current_provider
    return PROVIDER_DELTABADGER if deltabadger_available?

    AppConfig.market_data_provider
  end

  # A DB-selected deltabadger provider with no credentials behind it is the stale-hosted-DB
  # case: a container database carried over to a self-hosted install, still claiming a feed it
  # can no longer reach. Reporting it as configured sends every caller into a Result::Failure
  # instead of the CoinGecko setup step.
  #
  # Scoped to the non-ENV path on purpose. When MARKET_DATA_URL is injected the container is
  # hosted and this is not a question worth re-deciding — widening the check there would flip
  # a token-less tenant from "401s from the feed" to "back through the setup wizard", which is
  # a fleet-wide behavior change this does not need to make.
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

  # Docker-internal network-alias launchpad's hosted deploy passes as MARKET_DATA_URL (see
  # deltabadger-launchpad's config/deploy.yml) — fast for server-to-server calls but never
  # browser-reachable. data-api's own public host (Kamal-proxied, Cloudflare-proxied) serves the
  # same instance's static assets (e.g. /logos/*) directly to browsers. Serving those images is the
  # ONLY thing that host does for us — every API call goes over the Docker network — which is why
  # fronting it with Cloudflare is right rather than a misconfiguration. Any other configured
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
