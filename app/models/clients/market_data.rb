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
      response = connection.get('api/v1/indices')
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

  private

  def connection
    @connection ||= Faraday.new(url: @url, **OPTIONS) do |config|
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
