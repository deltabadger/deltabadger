class CoingeckoClient < ApplicationClient
  URL = 'https://pro-api.coingecko.com/api/v3'.freeze
  KEY = ENV.fetch('COINGECKO_API_KEY').freeze

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.headers = {
        'x-cg-pro-api-key': KEY
      }
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: true, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # https://docs.coingecko.com/reference/simple-price
  # @param coin_ids [Array] An array of coin ids to get all tickers.
  # @param vs_currencies [Array] An array of vs_currencies to get all prices.
  def simple_price(coin_ids, vs_currencies)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url 'simple/price'
        req.params['ids'] = coin_ids.join(',')
        req.params['vs_currencies'] = vs_currencies.join(',')
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.coingecko.com/reference/coins-id-market-chart-range
  # @param coin_id [String] The coin id
  # @param vs_currency [String] The target currency of market chart
  # @param from [Int] From date in UNIX timestamp
  # @param to [Int] To date in UNIX timestamp
  # @param interval [String] can be one of these values: '5m', 'hourly', 'daily'
  # @param precision [Int] or 'full' for full data
  def market_chart(coin_id, vs_currency, from:, to:, interval: 'daily', precision: 'full')
    with_rescue do
      response = self.class.connection.get do |req|
        req.url "coins/#{coin_id}/market_chart/range"
        req.params['vs_currency'] = vs_currency
        req.params['from'] = from.to_i
        req.params['to'] = to.to_i
        req.params['interval'] = interval if interval.present?
        req.params['precision'] = precision if precision.present?
      end
      Result::Success.new(response.body)
    end
  end
end
