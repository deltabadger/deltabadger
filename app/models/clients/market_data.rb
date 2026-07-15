class Clients::MarketData < Client
  def initialize(url:, token:)
    @url = url
    @token = token
  end

  def get_assets
    with_rescue do
      response = connection.get('api/v1/assets')
      Result::Success.new(response.body)
    end
  end

  def get_indices
    with_rescue do
      response = v2_connection.get('api/v2/indices')
      Result::Success.new(response.body)
    end
  end

  def get_stock_colors
    with_rescue do
      response = connection.get('api/v1/stock_colors')
      Result::Success.new(response.body)
    end
  end

  # The two bulk stock endpoints return large payloads (~11.6k assets / ~6.7k listings). Give them a
  # longer read window than the 30s default (Fix B) so a slow cold response doesn't time out; trading
  # clients keep the tighter global timeout. Server-side caching (data-api Fix D) makes the warm path fast.
  BULK_READ_TIMEOUT = 60

  def get_stocks
    with_rescue do
      response = v2_connection.get('api/v2/assets', { type: 'stock,etf', include: 'identifiers' }) do |req|
        req.options.timeout = BULK_READ_TIMEOUT
      end
      Result::Success.new(response.body)
    end
  end

  def get_alpaca_listings
    with_rescue do
      response = v2_connection.get('api/v2/listings', { venue_scheme: 'alpaca_exchange' }) do |req|
        req.options.timeout = BULK_READ_TIMEOUT
      end
      Result::Success.new(response.body)
    end
  end

  def get_alpaca_crypto_listings
    with_rescue do
      response = v2_connection.get('api/v2/listings', { venue: 'alpaca_crypto' }) do |req|
        req.options.timeout = BULK_READ_TIMEOUT
      end
      Result::Success.new(response.body)
    end
  end

  def get_tickers(exchange:)
    with_rescue do
      response = connection.get("api/v1/tickers/#{exchange}")
      Result::Success.new(response.body)
    end
  end

  def get_sync_status
    with_rescue do
      response = connection.get('api/v1/sync_status')
      Result::Success.new(response.body)
    end
  end

  def get_prices(coin_ids:, vs_currencies:)
    with_rescue do
      response = connection.get('api/v1/prices') do |req|
        req.params = { coin_ids: coin_ids.join(','), vs_currencies: vs_currencies.join(',') }
      end
      Result::Success.new(response.body)
    end
  end

  def get_exchange_rates
    with_rescue do
      response = connection.get('api/v1/exchange_rates')
      Result::Success.new(response.body)
    end
  end

  def get_historical_prices(coin_id:, currency:, from:, to:)
    with_rescue do
      response = connection.get('api/v1/historical_prices') do |req|
        req.params = { coin_id: coin_id, currency: currency, from: from.to_i, to: to.to_i }
      end
      Result::Success.new(response.body)
    end
  end

  private

  def connection
    @connection ||= build_connection
  end

  # Separate accessor so v2 callers (stocks, alpaca listings) can be stubbed independently
  # of v1 in tests. The wire protocol is the same; only the URL paths differ.
  def v2_connection
    @v2_connection ||= build_connection
  end

  def build_connection
    Faraday.new(url: @url, **OPTIONS) do |config|
      config.headers = {
        'Authorization' => "Bearer #{@token}",
        'Accept' => 'application/json'
      }
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end
end
