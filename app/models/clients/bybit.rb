class Clients::Bybit < Client
  # https://bybit-exchange.github.io/docs/v5/intro

  URL = 'https://api.bybit.com'.freeze
  RECV_WINDOW = '5000'.freeze

  def initialize(api_key: nil, api_secret: nil)
    super()
    @api_key = api_key
    @api_secret = api_secret
  end

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.proxy = ENV['PROXY_BYBIT'] if ENV['PROXY_BYBIT'].present?
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # https://bybit-exchange.github.io/docs/v5/asset/coin-info
  def get_coin_query_info
    with_rescue do
      params = {}
      response = self.class.connection.get do |req|
        req.url '/v5/asset/coin/query-info'
        req.headers = authenticated_headers('GET', params)
        req.params = params
      end
      Result::Success.new(response.body)
    end
  end

  # https://bybit-exchange.github.io/docs/v5/market/instrument
  # @param category [String] Product type: spot, linear, inverse, option
  # @param symbol [String]
  # @param status [String]
  # @param base_coin [String]
  # @param limit [Integer]
  # @param cursor [String]
  def instruments_info(category:, symbol: nil, status: nil, base_coin: nil, limit: nil, cursor: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/v5/market/instruments-info'
        req.headers = unauthenticated_headers
        req.params = {
          category: category,
          symbol: symbol,
          status: status,
          baseCoin: base_coin,
          limit: limit,
          cursor: cursor
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://bybit-exchange.github.io/docs/v5/market/tickers
  # @param category [String] Product type: spot, linear, inverse, option
  # @param symbol [String]
  # @param base_coin [String]
  # @param exp_date [String]
  def tickers(category:, symbol: nil, base_coin: nil, exp_date: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/v5/market/tickers'
        req.headers = unauthenticated_headers
        req.params = {
          category: category,
          symbol: symbol,
          baseCoin: base_coin,
          expDate: exp_date
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://bybit-exchange.github.io/docs/v5/market/orderbook
  # @param category [String] Product type: spot, linear, inverse, option
  # @param symbol [String]
  # @param limit [Integer]
  def orderbook(category:, symbol:, limit: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/v5/market/orderbook'
        req.headers = unauthenticated_headers
        req.params = {
          category: category,
          symbol: symbol,
          limit: limit
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://bybit-exchange.github.io/docs/v5/market/kline
  # @param category [String] Product type: spot, linear, inverse
  # @param symbol [String]
  # @param interval [String] Kline interval: 1,3,5,15,30,60,120,240,360,720,D,M,W
  # @param start [Integer] The start timestamp (ms)
  # @param end_time [Integer] The end timestamp (ms)
  # @param limit [Integer] Limit for data size per page. [1, 1000]. Default: 200
  def kline(category:, symbol:, interval:, start: nil, end_time: nil, limit: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/v5/market/kline'
        req.headers = unauthenticated_headers
        req.params = {
          category: category,
          symbol: symbol,
          interval: interval,
          start: start,
          end: end_time,
          limit: limit
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://bybit-exchange.github.io/docs/v5/account/wallet-balance
  # @param account_type [String] Account type: UNIFIED, CONTRACT, SPOT
  # @param coin [String]
  def wallet_balance(account_type:, coin: nil)
    with_rescue do
      params = {
        accountType: account_type,
        coin: coin
      }.compact
      response = self.class.connection.get do |req|
        req.url '/v5/account/wallet-balance'
        req.headers = authenticated_headers('GET', params)
        req.params = params
      end
      Result::Success.new(response.body)
    end
  end

  # https://bybit-exchange.github.io/docs/v5/order/open-order
  # @param category [String] Product type: spot, linear, inverse, option
  # @param order_id [String]
  # @param symbol [String]
  # @param order_link_id [String]
  def get_order(category:, order_id: nil, symbol: nil, order_link_id: nil)
    with_rescue do
      params = {
        category: category,
        orderId: order_id,
        symbol: symbol,
        orderLinkId: order_link_id
      }.compact
      response = self.class.connection.get do |req|
        req.url '/v5/order/realtime'
        req.headers = authenticated_headers('GET', params)
        req.params = params
      end
      Result::Success.new(response.body)
    end
  end

  # https://bybit-exchange.github.io/docs/v5/order/create-order
  # @param category [String] Product type: spot, linear, inverse, option
  # @param symbol [String]
  # @param side [String] Buy, Sell
  # @param order_type [String] Market, Limit
  # @param qty [String]
  # @param price [String]
  # @param time_in_force [String]
  # @param market_unit [String] baseCoin, quoteCoin
  # @param order_link_id [String]
  def create_order(
    category:,
    symbol:,
    side:,
    order_type:,
    qty:,
    price: nil,
    time_in_force: nil,
    market_unit: nil,
    order_link_id: nil
  )
    with_rescue do
      body = {
        category: category,
        symbol: symbol,
        side: side,
        orderType: order_type,
        qty: qty,
        price: price,
        timeInForce: time_in_force,
        marketUnit: market_unit,
        orderLinkId: order_link_id
      }.compact
      response = self.class.connection.post do |req|
        req.url '/v5/order/create'
        req.headers = authenticated_headers('POST', body)
        req.body = body
      end
      Result::Success.new(response.body)
    end
  end

  # https://bybit-exchange.github.io/docs/v5/order/cancel-order
  # @param category [String] Product type: spot, linear, inverse, option
  # @param symbol [String]
  # @param order_id [String]
  # @param order_link_id [String]
  def cancel_order(category:, symbol:, order_id: nil, order_link_id: nil)
    with_rescue do
      body = {
        category: category,
        symbol: symbol,
        orderId: order_id,
        orderLinkId: order_link_id
      }.compact
      response = self.class.connection.post do |req|
        req.url '/v5/order/cancel'
        req.headers = authenticated_headers('POST', body)
        req.body = body
      end
      Result::Success.new(response.body)
    end
  end

  # https://bybit-exchange.github.io/docs/v5/asset/withdraw
  # @param coin [String] Coin name (e.g., "BTC")
  # @param chain [String] Chain name
  # @param address [String] Withdrawal address
  # @param amount [String] Withdrawal amount
  # @param tag [String] Optional tag/memo
  # @param force_chain [Integer] 0 or 1
  def withdraw(coin:, chain:, address:, amount:, tag: nil, force_chain: nil)
    with_rescue do
      body = {
        coin: coin,
        chain: chain,
        address: address,
        amount: amount,
        tag: tag,
        forceChain: force_chain,
        timestamp: timestamp
      }.compact
      response = self.class.connection.post do |req|
        req.url '/v5/asset/withdraw/create'
        req.headers = authenticated_headers('POST', body)
        req.body = body
      end
      Result::Success.new(response.body)
    end
  end

  private

  def unauthenticated_headers
    {
      Accept: 'application/json',
      'Content-Type': 'application/json'
    }
  end

  def unauthenticated?
    @api_key.blank? || @api_secret.blank?
  end

  def authenticated_headers(method, params_or_body)
    return unauthenticated_headers if unauthenticated?

    ts = timestamp
    payload = if method == 'GET'
                query_string = Faraday::Utils.build_query(params_or_body)
                "#{ts}#{@api_key}#{RECV_WINDOW}#{query_string}"
              else
                body_string = params_or_body.to_json
                "#{ts}#{@api_key}#{RECV_WINDOW}#{body_string}"
              end

    signature = OpenSSL::HMAC.hexdigest('sha256', @api_secret, payload)

    {
      'X-BAPI-API-KEY': @api_key,
      'X-BAPI-SIGN': signature,
      'X-BAPI-TIMESTAMP': ts.to_s,
      'X-BAPI-RECV-WINDOW': RECV_WINDOW,
      Accept: 'application/json',
      'Content-Type': 'application/json'
    }
  end

  def timestamp
    (Time.now.utc.to_f * 1_000).to_i
  end
end
