class Clients::Binance < Client
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
  # @param limit [Integer] The value cannot be greater than 1000
  def candlestick_data(symbol:, interval:, start_time: nil, end_time: nil, time_zone: 0, limit: 500)
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

  # https://developers.binance.com/docs/binance-spot-api-docs/rest-api/account-endpoints#account-information-user_data
  # @param omit_zero_balances [Boolean] When set to true, emits only the non-zero balances of an account
  # @param recv_window [Integer] The value cannot be greater than 60000
  def account_information(omit_zero_balances: false, recv_window: 5000)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/account'
        req.headers = headers
        req.params = {
          omitZeroBalances: omit_zero_balances,
          recvWindow: recv_window,
          timestamp: timestamp
        }.compact
        req.params[:signature] = hmac_signature(req.params)
      end
      Result::Success.new(response.body)
    end
  end

  # https://developers.binance.com/docs/binance-spot-api-docs/rest-api/account-endpoints#account-trade-list-user_data
  # @param symbol [String]
  # @param order_id [Integer]
  # @param start_time [Integer]
  # @param end_time [Integer]
  # @param from_id [Integer]
  # @param limit [Integer] The value cannot be greater than 1000
  # @param recv_window [Integer] The value cannot be greater than 60000
  def account_trade_list(symbol:, order_id: nil, start_time: nil, end_time: nil, from_id: nil, limit: 500, recv_window: 5000)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/myTrades'
        req.headers = headers
        req.params = {
          symbol: symbol,
          orderId: order_id,
          startTime: start_time,
          endTime: end_time,
          fromId: from_id,
          limit: limit,
          recvWindow: recv_window,
          timestamp: timestamp
        }.compact
        req.params[:signature] = hmac_signature(req.params)
      end
      Result::Success.new(response.body)
    end
  end

  # https://developers.binance.com/docs/binance-spot-api-docs/rest-api/account-endpoints#query-order-user_data
  # @param symbol [String]
  # @param order_id [Integer]
  # @param orig_client_order_id [String]
  # @param recv_window [Integer] The value cannot be greater than 60000
  def query_order(symbol:, order_id: nil, orig_client_order_id: nil, recv_window: 5000)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/order'
        req.headers = headers
        req.params = {
          symbol: symbol,
          orderId: order_id,
          origClientOrderId: orig_client_order_id,
          recvWindow: recv_window,
          timestamp: timestamp
        }.compact
        req.params[:signature] = hmac_signature(req.params)
      end
      Result::Success.new(response.body)
    end
  end

  # https://developers.binance.com/docs/binance-spot-api-docs/rest-api/account-endpoints#all-orders-user_data
  # @param symbol [String]
  # @param order_id [Integer]
  # @param start_time [Integer]
  # @param end_time [Integer]
  # @param limit [Integer] The value cannot be greater than 1000
  # @param recv_window [Integer] The value cannot be greater than 60000
  def all_orders(symbol:, order_id: nil, start_time: nil, end_time: nil, limit: 500, recv_window: 5000)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/allOrders'
        req.headers = headers
        req.params = {
          symbol: symbol,
          orderId: order_id,
          startTime: start_time,
          endTime: end_time,
          limit: limit,
          recvWindow: recv_window,
          timestamp: timestamp
        }.compact
        req.params[:signature] = hmac_signature(req.params)
      end
      Result::Success.new(response.body)
    end
  end

  # https://developers.binance.com/docs/binance-spot-api-docs/rest-api/trading-endpoints#new-order-trade
  # @param symbol [String]
  # @param side [String]
  # @param type [String]
  # @param time_in_force [String]
  # @param quantity [String]
  # @param quote_order_quantity [String]
  # @param price [String]
  # @param new_client_order_id [String]
  # @param strategy_id [Integer]
  # @param strategy_type [Integer]
  # @param stop_price [String]
  # @param trailing_delta [Integer]
  # @param iceberg_qty [String]
  # @param new_order_resp_type [String]
  # @param self_trade_prevention_mode [String]
  # @param recv_window [Integer] The value cannot be greater than 60000
  def new_order(
    symbol:,
    side:,
    type:,
    time_in_force: nil,
    quantity: nil,
    quote_order_qty: nil,
    price: nil,
    new_client_order_id: nil,
    strategy_id: nil,
    strategy_type: nil,
    stop_price: nil,
    trailing_delta: nil,
    iceberg_qty: nil,
    new_order_resp_type: nil,
    self_trade_prevention_mode: nil,
    recv_window: 5000
  )
    with_rescue do
      response = self.class.connection.post do |req|
        req.url '/api/v3/order'
        req.headers = headers
        req.params = {
          symbol: symbol,
          side: side,
          type: type,
          timeInForce: time_in_force,
          quantity: quantity,
          quoteOrderQty: quote_order_qty,
          price: price,
          newClientOrderId: new_client_order_id,
          strategyId: strategy_id,
          strategyType: strategy_type,
          stopPrice: stop_price,
          trailingDelta: trailing_delta,
          icebergQty: iceberg_qty,
          newOrderRespType: new_order_resp_type,
          selfTradePreventionMode: self_trade_prevention_mode,
          recvWindow: recv_window,
          timestamp: timestamp
        }.compact
        req.params[:signature] = hmac_signature(req.params)
      end
      Result::Success.new(response.body)
    end
  end

  # https://developers.binance.com/docs/binance-spot-api-docs/rest-api/trading-endpoints#test-new-order-trade
  # @param symbol [String]
  # @param side [String]
  # @param type [String]
  # @param time_in_force [String]
  # @param quantity [String]
  # @param quote_order_quantity [String]
  # @param price [String]
  # @param new_client_order_id [String]
  # @param strategy_id [Integer]
  # @param strategy_type [Integer]
  # @param stop_price [String]
  # @param trailing_delta [Integer]
  # @param iceberg_qty [String]
  # @param new_order_resp_type [String]
  # @param self_trade_prevention_mode [String]
  # @param recv_window [Integer] The value cannot be greater than 60000
  def test_new_order(
    symbol:,
    side:,
    type:,
    time_in_force: nil,
    quantity: nil,
    quote_order_quantity: nil,
    price: nil,
    new_client_order_id: nil,
    strategy_id: nil,
    strategy_type: nil,
    stop_price: nil,
    trailing_delta: nil,
    iceberg_qty: nil,
    new_order_resp_type: nil,
    self_trade_prevention_mode: nil,
    recv_window: 5000,
    compute_commission_rates: false
  )
    with_rescue do
      response = self.class.connection.post do |req|
        req.url '/api/v3/order/test'
        req.headers = headers
        req.params = {
          symbol: symbol,
          side: side,
          type: type,
          timeInForce: time_in_force,
          quantity: quantity,
          quoteOrderQuantity: quote_order_quantity,
          price: price,
          newClientOrderId: new_client_order_id,
          strategyId: strategy_id,
          strategyType: strategy_type,
          stopPrice: stop_price,
          trailingDelta: trailing_delta,
          icebergQty: iceberg_qty,
          newOrderRespType: new_order_resp_type,
          selfTradePreventionMode: self_trade_prevention_mode,
          recvWindow: recv_window,
          computeCommissionRates: compute_commission_rates,
          timestamp: timestamp
        }.compact
        req.params[:signature] = hmac_signature(req.params)
      end
      Result::Success.new(response.body)
    end
  end

  # https://developers.binance.com/docs/binance-spot-api-docs/rest-api/trading-endpoints#cancel-order-trade
  # @param symbol [String]
  # @param order_id [Integer]
  # @param orig_client_order_id [String]
  # @param new_client_order_id [String]
  # @param cancel_restrictions [String]
  # @param recv_window [Integer] The value cannot be greater than 60000
  def cancel_order(
    symbol:,
    order_id: nil,
    orig_client_order_id: nil,
    new_client_order_id: nil,
    cancel_restrictions: nil,
    recv_window: 5000
  )
    with_rescue do
      response = self.class.connection.delete do |req|
        req.url '/api/v3/order'
        req.headers = headers
        req.params = {
          symbol: symbol,
          orderId: order_id,
          origClientOrderId: orig_client_order_id,
          newClientOrderId: new_client_order_id,
          cancelRestrictions: cancel_restrictions,
          recvWindow: recv_window,
          timestamp: timestamp
        }.compact
        req.params[:signature] = hmac_signature(req.params)
      end
      Result::Success.new(response.body)
    end
  end

  # https://developers.binance.com/docs/wallet/account/api-key-permission#api-description
  # @param recv_window [Integer] The value cannot be greater than 60000
  def api_description(recv_window: 5000)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/sapi/v1/account/apiRestrictions'
        req.headers = headers
        req.params = {
          recvWindow: recv_window,
          timestamp: timestamp
        }.compact
        req.params[:signature] = hmac_signature(req.params)
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

  def timestamp
    (Time.now.utc.to_f * 1_000).to_i
  end

  def hmac_signature(params)
    return if @api_secret.blank?

    query = Faraday::Utils.build_query(params)
    OpenSSL::HMAC.hexdigest('sha256', @api_secret, query)
  end
end
