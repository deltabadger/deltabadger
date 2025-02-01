class BinanceClient < ApplicationClient
  EU_PROXY_IP = ENV.fetch('EU_PROXY_IP', nil).freeze

  EU_URL_BASE = 'https://api.binance.com/api/v3'.freeze
  US_URL_BASE = 'https://api.binance.us/api/v3'.freeze
  EU_WITHDRAWAL_URL_BASE = 'https://api.binance.com/sapi/v1'.freeze
  US_WITHDRAWAL_URL_BASE = 'https://api.binance.us/wapi/v3'.freeze
  EXPIRE_TIME = ENV.fetch('DEFAULT_MARKET_CACHING_TIME', 60 * 60).to_i.freeze

  AddTimestamp = Struct.new(:app) do
    def call(env)
      timestamp = DateTime.now.strftime('%Q')
      env.url.query = "#{env.url.query}&timestamp=#{timestamp}"
      app.call env
    end
  end

  AddSignature = Struct.new(:app, :api_secret) do
    def call(env)
      signature = OpenSSL::HMAC.hexdigest('sha256', api_secret, env.url.query)
      env.url.query = "#{env.url.query}&signature=#{signature}"
      app.call env
    end
  end

  def initialize(api_key: nil, api_secret: nil, url_base: EU_URL_BASE, caching: false)
    super()
    @api_key = api_key
    @api_secret = api_secret
    @url_base = url_base
    @caching = caching
  end

  def use_proxy?
    @url_base.in?([EU_URL_BASE, EU_WITHDRAWAL_URL_BASE]) && EU_PROXY_IP.present?
  end

  def connection
    @connection ||= Faraday.new(url: @url_base, **OPTIONS) do |config|
      if @api_key && @api_secret
        config.headers['X-MBX-APIKEY'] = @api_key
        config.use AddTimestamp
        config.use AddSignature, @api_secret
      end
      config.proxy EU_PROXY_IP if use_proxy?
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.use :manual_cache, expires_in: EXPIRE_TIME.seconds, store: Rails.cache, logger: Rails.logger if @caching
      config.adapter Faraday.default_adapter
    end
  end

  def exchange_info
    # TODO: in the wrapper use global caching
    with_rescue do
      response = connection.get do |req|
        req.url 'exchangeInfo'
      end
      Result::Success.new(response.body)
    end
  end

  # @param symbol [String] The symbol to get the order book ticker
  # @param symbols [Array] An array of symbols to get the order book ticker
  # use only one of the params
  def symbol_order_book_ticker(symbol: nil, symbols: nil)
    with_rescue do
      response = connection.get do |req|
        req.url 'ticker/bookTicker'
        req.params['symbol'] = symbol if symbol
        req.params['symbols'] = symbols.to_s.gsub(' ', '') if symbols
      end
      Result::Success.new(response.body)
    end
  end

  # @param recv_window [Integer] The value of the recvWindow. Must be less than 60000
  def account(recv_window: nil)
    with_rescue do
      response = connection.get do |req|
        req.url 'account'
        req.params['recvWindow'] = recv_window if recv_window
      end
      Result::Success.new(response.body)
    end
  end

  def order_test(
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
    recv_window: nil,
    compute_commission_rates: nil
  )
    with_rescue do
      response = connection.post do |req|
        req.url 'order/test'
        req.params['symbol'] = symbol
        req.params['side'] = side
        req.params['type'] = type
        req.params['timeInForce'] = time_in_force if time_in_force
        req.params['quantity'] = quantity if quantity
        req.params['quoteOrderQty'] = quote_order_qty if quote_order_qty
        req.params['price'] = price if price
        req.params['newClientOrderId'] = new_client_order_id if new_client_order_id
        req.params['strategyId'] = strategy_id if strategy_id
        req.params['strategyType'] = strategy_type if strategy_type
        req.params['stopPrice'] = stop_price if stop_price
        req.params['trailingDelta'] = trailing_delta if trailing_delta
        req.params['icebergQty'] = iceberg_qty if iceberg_qty
        req.params['newOrderRespType'] = new_order_resp_type if new_order_resp_type
        req.params['selfTradePreventionMode'] = self_trade_prevention_mode if self_trade_prevention_mode
        req.params['recvWindow'] = recv_windo if recv_window
        req.params['computeCommissionRates'] = compute_commission_rates if compute_commission_rates
      end
      Result::Success.new(response.body)
    end
  end

  def order(
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
    recv_window: nil
  )
    with_rescue do
      response = connection.post do |req|
        req.url 'order'
        req.params['symbol'] = symbol
        req.params['side'] = side
        req.params['type'] = type
        req.params['timeInForce'] = time_in_force if time_in_force
        req.params['quantity'] = quantity if quantity
        req.params['quoteOrderQty'] = quote_order_qty if quote_order_qty
        req.params['price'] = price if price
        req.params['newClientOrderId'] = new_client_order_id if new_client_order_id
        req.params['strategyId'] = strategy_id if strategy_id
        req.params['strategyType'] = strategy_type if strategy_type
        req.params['stopPrice'] = stop_price if stop_price
        req.params['trailingDelta'] = trailing_delta if trailing_delta
        req.params['icebergQty'] = iceberg_qty if iceberg_qty
        req.params['newOrderRespType'] = new_order_resp_type if new_order_resp_type
        req.params['selfTradePreventionMode'] = self_trade_prevention_mode if self_trade_prevention_mode
        req.params['recvWindow'] = recv_windo if recv_window
      end
      Result::Success.new(response.body)
    end
  end
end
