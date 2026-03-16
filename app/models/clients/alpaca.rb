class Clients::Alpaca < Client
  # https://docs.alpaca.markets/docs/trading-api

  TRADING_URL = 'https://api.alpaca.markets'.freeze
  PAPER_TRADING_URL = 'https://paper-api.alpaca.markets'.freeze
  DATA_URL = 'https://data.alpaca.markets'.freeze

  def initialize(api_key: nil, api_secret: nil, paper: false)
    super()
    @api_key = api_key
    @api_secret = api_secret
    @paper = paper
  end

  def trading_connection
    @trading_connection ||= Faraday.new(url: @paper ? PAPER_TRADING_URL : TRADING_URL, **OPTIONS) do |config|
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  def data_connection
    @data_connection ||= Faraday.new(url: DATA_URL, **OPTIONS) do |config|
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # https://docs.alpaca.markets/reference/getaccount-1
  def get_account
    with_rescue do
      response = trading_connection.get do |req|
        req.url '/v2/account'
        req.headers = authenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.alpaca.markets/reference/getallopenpositions-1
  def get_positions
    with_rescue do
      response = trading_connection.get do |req|
        req.url '/v2/positions'
        req.headers = authenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.alpaca.markets/reference/get-v2-assets
  # @param status [String] 'active' or 'inactive'
  # @param asset_class [String] 'us_equity' or 'crypto'
  def get_assets(status: 'active', asset_class: 'us_equity')
    with_rescue do
      response = trading_connection.get do |req|
        req.url '/v2/assets'
        req.headers = authenticated_headers
        req.params = { status: status, asset_class: asset_class }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.alpaca.markets/reference/getassetbyidorsymbol
  # @param symbol [String] asset symbol (e.g., 'AAPL')
  def get_asset(symbol:)
    with_rescue do
      response = trading_connection.get do |req|
        req.url "/v2/assets/#{symbol}"
        req.headers = authenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.alpaca.markets/reference/postorder
  # @param symbol [String] asset symbol
  # @param notional [String] dollar amount for market orders
  # @param qty [String] share quantity for limit orders
  # @param side [String] 'buy' or 'sell'
  # @param type [String] 'market' or 'limit'
  # @param time_in_force [String] 'day', 'gtc', etc.
  # @param limit_price [String] limit price for limit orders
  def create_order(symbol:, side:, type:, time_in_force:, notional: nil, qty: nil, limit_price: nil)
    with_rescue do
      body = {
        symbol: symbol,
        side: side,
        type: type,
        time_in_force: time_in_force,
        notional: notional,
        qty: qty,
        limit_price: limit_price
      }.compact
      response = trading_connection.post do |req|
        req.url '/v2/orders'
        req.headers = authenticated_headers
        req.body = body
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.alpaca.markets/reference/getorderbyorderid
  # @param order_id [String] order ID
  def get_order(order_id:)
    with_rescue do
      response = trading_connection.get do |req|
        req.url "/v2/orders/#{order_id}"
        req.headers = authenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.alpaca.markets/reference/getallorders
  # @param status [String] 'open', 'closed', or 'all'
  # @param limit [Integer] max number of orders (default 50, max 500)
  # @param after [String] RFC 3339 timestamp
  # @param until_time [String] RFC 3339 timestamp
  # @param direction [String] 'asc' or 'desc'
  def list_orders(status: 'open', limit: 50, after: nil, until_time: nil, direction: nil)
    with_rescue do
      response = trading_connection.get do |req|
        req.url '/v2/orders'
        req.headers = authenticated_headers
        req.params = { status: status, limit: limit, after: after, until: until_time, direction: direction }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.alpaca.markets/reference/deleteorderbyorderid
  # @param order_id [String] order ID
  def cancel_order(order_id:)
    with_rescue do
      response = trading_connection.delete do |req|
        req.url "/v2/orders/#{order_id}"
        req.headers = authenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.alpaca.markets/reference/stocklatestquote
  # @param symbol [String] stock symbol
  def get_latest_quote(symbol:)
    with_rescue do
      response = data_connection.get do |req|
        req.url "/v2/stocks/#{symbol}/quotes/latest"
        req.headers = authenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.alpaca.markets/reference/stocklatesttrade-1
  # @param symbol [String] stock symbol
  def get_latest_trade(symbol:)
    with_rescue do
      response = data_connection.get do |req|
        req.url "/v2/stocks/#{symbol}/trades/latest"
        req.headers = authenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.alpaca.markets/reference/stockbars-1
  # @param symbol [String] stock symbol
  # @param timeframe [String] e.g., '1Day', '1Hour'
  # @param start_time [String] RFC 3339 timestamp
  # @param end_time [String] RFC 3339 timestamp
  def get_bars(symbol:, timeframe:, start_time: nil, end_time: nil)
    with_rescue do
      response = data_connection.get do |req|
        req.url "/v2/stocks/#{symbol}/bars"
        req.headers = authenticated_headers
        req.params = { timeframe: timeframe, start: start_time, end: end_time }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.alpaca.markets/reference/getclock-1
  def get_clock
    with_rescue do
      response = trading_connection.get do |req|
        req.url '/v2/clock'
        req.headers = authenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  private

  def authenticated_headers
    {
      'APCA-API-KEY-ID' => @api_key.to_s,
      'APCA-API-SECRET-KEY' => @api_secret.to_s,
      'Accept' => 'application/json',
      'Content-Type' => 'application/json'
    }
  end
end
