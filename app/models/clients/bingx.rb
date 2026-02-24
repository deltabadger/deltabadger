class Clients::Bingx < Client
  # https://bingx-api.github.io/docs/

  URL = 'https://open-api.bingx.com'.freeze

  def initialize(api_key: nil, api_secret: nil)
    super()
    @api_key = api_key
    @api_secret = api_secret
  end

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.proxy = ENV['PROXY_BINGX'] if ENV['PROXY_BINGX'].present?
      config.request :json
      config.response :json, content_type: //
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # https://bingx-api.github.io/docs/#/spot/market-api.html#Query%20Symbols
  def get_symbols
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/openApi/spot/v1/common/symbols'
        req.headers = unauthenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://bingx-api.github.io/docs/#/spot/market-api.html#24hr%20Ticker%20Price%20Change%20Statistics
  # @param symbol [String] optional trading pair
  def get_ticker(symbol: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/openApi/spot/v1/ticker/24hr'
        req.headers = unauthenticated_headers
        req.params = { symbol: symbol }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://bingx-api.github.io/docs/#/spot/market-api.html#Order%20Book
  # @param symbol [String] trading pair
  # @param limit [Integer] depth limit
  def get_depth(symbol:, limit: 20)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/openApi/spot/v1/market/depth'
        req.headers = unauthenticated_headers
        req.params = { symbol: symbol, limit: limit }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://bingx-api.github.io/docs/#/spot/market-api.html#K-Line%20Data
  # @param symbol [String] trading pair
  # @param interval [String] kline interval
  # @param start_time [Integer] start time in milliseconds
  # @param end_time [Integer] end time in milliseconds
  # @param limit [Integer] max number of candles
  def get_klines(symbol:, interval:, start_time: nil, end_time: nil, limit: 1000)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/openApi/spot/v2/market/kline'
        req.headers = unauthenticated_headers
        req.params = {
          symbol: symbol,
          interval: interval,
          startTime: start_time,
          endTime: end_time,
          limit: limit
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://bingx-api.github.io/docs/#/spot/account-api.html#Query%20Assets
  def get_balances
    with_rescue do
      params = { timestamp: timestamp }
      params[:signature] = hmac_signature(params)
      response = self.class.connection.get do |req|
        req.url '/openApi/spot/v1/account/balance'
        req.headers = authenticated_headers
        req.params = params
      end
      Result::Success.new(response.body)
    end
  end

  # https://bingx-api.github.io/docs/#/spot/trade-api.html#Create%20an%20Order
  # @param symbol [String] trading pair
  # @param side [String] BUY or SELL
  # @param type [String] MARKET or LIMIT
  # @param quantity [String] base amount
  # @param quote_order_qty [String] quote amount for market buy
  # @param price [String] price for limit orders
  # @param time_in_force [String] time in force (GTC, IOC, FOK)
  def create_order(symbol:, side:, type:, quantity: nil, quote_order_qty: nil, price: nil, time_in_force: nil)
    with_rescue do
      params = {
        symbol: symbol,
        side: side,
        type: type,
        quantity: quantity,
        quoteOrderQty: quote_order_qty,
        price: price,
        timeInForce: time_in_force,
        timestamp: timestamp
      }.compact
      params[:signature] = hmac_signature(params)
      response = self.class.connection.post do |req|
        req.url '/openApi/spot/v2/trade/order'
        req.headers = authenticated_headers
        req.params = params
      end
      Result::Success.new(response.body)
    end
  end

  # https://bingx-api.github.io/docs/#/spot/trade-api.html#Query%20Orders
  # @param symbol [String] trading pair
  # @param order_id [String] order ID
  def get_order(symbol:, order_id:)
    with_rescue do
      params = {
        symbol: symbol,
        orderId: order_id,
        timestamp: timestamp
      }.compact
      params[:signature] = hmac_signature(params)
      response = self.class.connection.get do |req|
        req.url '/openApi/spot/v1/trade/query'
        req.headers = authenticated_headers
        req.params = params
      end
      Result::Success.new(response.body)
    end
  end

  # https://bingx-api.github.io/docs/#/spot/trade-api.html#Cancel%20an%20Order
  # @param symbol [String] trading pair
  # @param order_id [String] order ID
  def cancel_order(symbol:, order_id:)
    with_rescue do
      params = {
        symbol: symbol,
        orderId: order_id,
        timestamp: timestamp
      }.compact
      params[:signature] = hmac_signature(params)
      response = self.class.connection.post do |req|
        req.url '/openApi/spot/v1/trade/cancel'
        req.headers = authenticated_headers
        req.params = params
      end
      Result::Success.new(response.body)
    end
  end

  # https://bingx-api.github.io/docs/#/spot/account-api.html#Query%20All%20Coins
  def get_all_coins_info
    with_rescue do
      params = { timestamp: timestamp }
      params[:signature] = hmac_signature(params)
      response = self.class.connection.get do |req|
        req.url '/openApi/wallets/v1/capital/config/getall'
        req.headers = authenticated_headers
        req.params = params
      end
      Result::Success.new(response.body)
    end
  end

  # https://bingx-api.github.io/docs/#/spot/account-api.html#Withdraw
  # @param coin [String] Coin name (e.g., "BTC")
  # @param address [String] Withdrawal address
  # @param amount [String] Withdrawal amount
  # @param network [String] Optional network name
  # @param address_tag [String] Optional tag/memo
  # @param wallet_type [Integer] 1 for fund account, 2 for standard account
  def withdraw(coin:, address:, amount:, network: nil, address_tag: nil, wallet_type: 1)
    with_rescue do
      params = {
        coin: coin,
        address: address,
        amount: amount,
        network: network,
        addressTag: address_tag,
        walletType: wallet_type,
        timestamp: timestamp
      }.compact
      params[:signature] = hmac_signature(params)
      response = self.class.connection.post do |req|
        req.url '/openApi/wallets/v1/capital/withdraw/apply'
        req.headers = authenticated_headers
        req.params = params
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

  def authenticated_headers
    return unauthenticated_headers if unauthenticated?

    {
      'X-BX-APIKEY': @api_key,
      Accept: 'application/json',
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
