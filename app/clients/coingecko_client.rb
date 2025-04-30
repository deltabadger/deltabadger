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

  # https://docs.coingecko.com/reference/coins-list
  # @param include_platform [Boolean] Include platform and token's contract addresses, default: false
  # @param status [String] Filter by status of coins, default: active
  def coins_list(include_platform: false, status: 'active')
    with_rescue do
      response = self.class.connection.get do |req|
        req.url 'coins/list'
        req.params = {
          include_platform: include_platform,
          status: status
        }
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
  def coins_list_with_market_data(
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

  # https://docs.coingecko.com/reference/coins-id
  # @param id [String] The coin id
  # @param localization [Boolean] Include all the localized languages in the response, default: true
  # @param tickers [Boolean] Include tickers data, default: true
  # @param market_data [Boolean] Include market data, default: true
  # @param community_data [Boolean] Include community data, default: true
  # @param developer_data [Boolean] Include developer data, default: true
  # @param sparkline [Boolean] Include sparkline 7 days data, default: false
  def coin_data_by_id(
    id:,
    localization: true,
    tickers: true,
    market_data: true,
    community_data: true,
    developer_data: true,
    sparkline: false
  )
    with_rescue do
      response = self.class.connection.get do |req|
        req.url "coins/#{id}"
        req.params = {
          localization: localization,
          tickers: tickers,
          market_data: market_data,
          community_data: community_data,
          developer_data: developer_data,
          sparkline: sparkline
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.coingecko.com/reference/simple-price
  # @param coin_ids [Array<String>] An array of coin ids to get all tickers.
  # @param vs_currencies [Array<String>] An array of vs_currencies to get all prices.
  # @param include_market_cap [Boolean] Include market capitalization, default: false
  # @param include_24hr_vol [Boolean] Include 24hr volume, default: false
  # @param include_24hr_change [Boolean] Include 24hr change, default: false
  # @param include_last_updated_at [Boolean] Include last updated price time in UNIX, default: false
  # @param precision [String] Decimal place for currency price value, default: 'full'
  def coin_price_by_ids(
    coin_ids:,
    vs_currencies:,
    include_market_cap: false,
    include_24hr_vol: false,
    include_24hr_change: false,
    include_last_updated_at: false,
    precision: 'full'
  )
    with_rescue do
      response = self.class.connection.get do |req|
        req.url 'simple/price'
        req.params = {
          ids: coin_ids.join(','),
          vs_currencies: vs_currencies.join(','),
          include_market_cap: include_market_cap,
          include_24hr_vol: include_24hr_vol,
          include_24hr_change: include_24hr_change,
          include_last_updated_at: include_last_updated_at,
          precision: precision
        }.compact
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
  def coin_historical_chart_data_within_time_range_by_id(
    coin_id:,
    vs_currency:,
    from:,
    to:,
    interval: 'daily',
    precision: 'full'
  )
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

  # https://docs.coingecko.com/reference/exchanges-id
  # @param id [String] The exchange id
  def exchange_data_by_id(id:)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url "exchanges/#{id}"
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.coingecko.com/reference/exchanges-id-tickers
  # @param id [String] The exchange id
  # @param coin_ids [Array<String>] Filter tickers by coin IDs
  # @param include_exchange_logo [Boolean] Include exchange logo, default: false
  # @param page [Int] Page through results, default: 1
  # @param depth [Boolean] Include 2% orderbook depth (Example: cost_to_move_up_usd & cost_to_move_down_usd), default: false
  # @param order [String] Use this to sort the order of responses, default: trust_score_desc
  def exchange_tickers_by_id(
    id:,
    coin_ids: nil,
    include_exchange_logo: false,
    page: 1,
    depth: false,
    order: 'trust_score_desc'
  )
    with_rescue do
      response = self.class.connection.get do |req|
        req.url "exchanges/#{id}/tickers"
        req.params = {
          coin_ids: coin_ids&.join(','),
          include_exchange_logo: include_exchange_logo,
          page: page,
          depth: depth,
          order: order
        }.compact
      end
      Result::Success.new(response.body)
    end
  end
end
