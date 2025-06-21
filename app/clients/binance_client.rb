class BinanceClient < ApplicationClient
  # https://developers.binance.com/docs/binance-spot-api-docs/rest-api

  URL = 'https://api.binance.com'.freeze
  PROXY = ENV['EU_HTTPS_PROXY'].present? ? "https://#{ENV['EU_HTTPS_PROXY']}".freeze : nil

  def initialize(api_key: nil, api_secret: nil)
    super()
    @api_key = api_key
    @api_secret = api_secret
  end

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.proxy = PROXY if PROXY.present?
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # https://developers.binance.com/docs/binance-spot-api-docs/rest-api/general-endpoints#exchange-information
  # @param symbol [String]
  # @param symbols [Array<String>]
  # @param permissions [Array<String>] Supports single or multiple values (e.g. SPOT, ["MARGIN","LEVERAGED"]). This cannot be used in combination with symbol or symbols.
  # @param show_permission_sets [Boolean] Controls whether the content of the permissionSets field is populated or not. Defaults to true
  # @param symbol_status [String] Filters symbols that have this tradingStatus. Valid values: TRADING, HALT, BREAK. Cannot be used in combination with symbols or symbol.
  def exchange_information(
    symbol: nil,
    symbols: nil,
    permissions: nil,
    show_permission_sets: nil,
    symbol_status: nil
  )
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/exchangeInfo'
        req.headers = headers
        req.params = {
          symbol: symbol,
          symbols: symbols.presence&.to_json,
          permissions: permissions.presence&.to_json,
          showPermissionSets: show_permission_sets,
          symbolStatus: symbol_status
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://developers.binance.com/docs/binance-spot-api-docs/rest-api/market-data-endpoints#symbol-price-ticker
  # @param symbol [String]
  # @param symbols [Array<String>]
  # Parameter symbol and symbols cannot be used in combination.
  # If neither parameter is sent, prices for all symbols will be returned in an array.
  def symbol_price_ticker(symbol: nil, symbols: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/ticker/price'
        req.headers = headers
        req.params = {
          symbol: symbol,
          symbols: symbols.presence&.to_json
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://developers.binance.com/docs/binance-spot-api-docs/rest-api/market-data-endpoints#symbol-order-book-ticker
  # @param symbol [String]
  # @param symbols [Array<String>]
  # Parameter symbol and symbols cannot be used in combination.
  # If neither parameter is sent, prices for all symbols will be returned in an array.
  def symbol_order_book_ticker(symbol: nil, symbols: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/ticker/bookTicker'
        req.headers = headers
        req.params = {
          symbol: symbol,
          symbols: symbols.presence&.to_json
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://developers.binance.com/docs/binance-spot-api-docs/rest-api/market-data-endpoints#klinecandlestick-data
  # @param symbol [String]
  # @param interval [String]
  # @param start_time [Integer]
  # @param end_time [Integer]
  # @param time_zone [Integer] Hours and minutes (e.g. -1:00, 05:45) OR only hours (e.g. 0, 8, 4). Accepted range is strictly [-12:00 to +14:00] inclusive
  # @param limit [Integer]
  def candlestick_data(symbol:, interval:, start_time: nil, end_time: nil, time_zone: 0, limit: 1000)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/klines'
        req.headers = headers
        req.params = {
          symbol: symbol,
          interval: interval,
          startTime: start_time,
          endTime: end_time,
          timeZone: time_zone,
          limit: limit
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  private

  def headers
    unauthenticated? ? unauthenticated_headers : authenticated_headers
  end

  def unauthenticated?
    @api_key.blank? || @api_secret.blank?
  end

  def unauthenticated_headers
    {
      'Accept': 'application/json',
      'Content-Type': 'application/json'
    }
  end

  def authenticated_headers
    {
      'X-MBX-APIKEY': @api_key,
      'Accept': 'application/json',
      'Content-Type': 'application/json'
    }
  end
end
