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
  # @param coin_ids [Array<String>] An array of coin ids to get all tickers.
  # @param vs_currencies [Array<String>] An array of vs_currencies to get all prices.
  def simple_price(coin_ids, vs_currencies)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url 'simple/price'
        req.params = {
          ids: coin_ids.join(','),
          vs_currencies: vs_currencies.join(',')
        }
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
        req.params = {
          vs_currency: vs_currency,
          from: from.to_i,
          to: to.to_i,
          interval: interval,
          precision: precision
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.coingecko.com/reference/coins-markets
  # @param vs_currency [String] The target currency of market chart
  # @param ids [Array<String>] coins' ids, comma-separated if querying more than 1 coin.
  # @param category [String] Filter based on coins' category.
  # @param order [String] Sort result by field, default: market_cap_desc
  # @param per_page [Int] Total results per page, default: 100
  #                       Valid values: 1...250
  # @param page [Int] Page through results, default: 1.
  # @param sparkline [Boolean] Whether to include sparkline 7 days data.
  # @param price_change_percentage [String] Include price change percentage timeframe,
  #                                         comma-separated if query more than 1 price
  #                                         change percentage timeframe
  # Valid values: 1h, 24h, 7d, 14d, 30d, 200d, 1y.
  # @param locale [String] The locale of the coin.
  # @param precision [String] Decimal place for currency price value.
  def coins_markets(
    vs_currency:,
    ids: nil,
    category: nil,
    order: 'market_cap_desc',
    per_page: 100,
    page: 1,
    sparkline: false,
    price_change_percentage: nil,
    locale: 'en',
    precision: 'full'
  )
    with_rescue do
      response = self.class.connection.get do |req|
        req.url 'coins/markets'
        ids = ids.join(',') if ids.present?
        req.params = {
          vs_currency: vs_currency,
          ids: ids,
          category: category,
          order: order,
          per_page: per_page,
          page: page,
          sparkline: sparkline,
          price_change_percentage: price_change_percentage,
          locale: locale,
          precision: precision
        }.compact
      end
      Result::Success.new(response.body)
    end
  end
end
