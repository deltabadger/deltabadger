class Clients::Mexc < Client
  # https://mexcdevelop.github.io/apidocs/spot_v3_en/

  URL = 'https://api.mexc.com'.freeze

  def initialize(api_key: nil, api_secret: nil)
    super()
    @api_key = api_key
    @api_secret = api_secret
  end

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.proxy = ENV['PROXY_MEXC'] if ENV['PROXY_MEXC'].present?
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # https://mexcdevelop.github.io/apidocs/spot_v3_en/#query-the-currency-information
  def get_all_coins_information
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/capital/config/getall'
        req.headers = headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://mexcdevelop.github.io/apidocs/spot_v3_en/#exchange-information
  def exchange_information
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/exchangeInfo'
        req.headers = headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://mexcdevelop.github.io/apidocs/spot_v3_en/#symbol-price-ticker
  # @param symbol [String]
  def symbol_price_ticker(symbol: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/ticker/price'
        req.headers = headers
        req.params = {
          symbol: symbol
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://mexcdevelop.github.io/apidocs/spot_v3_en/#symbol-order-book-ticker
  # @param symbol [String]
  def symbol_order_book_ticker(symbol: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/ticker/bookTicker'
        req.headers = headers
        req.params = {
          symbol: symbol
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://mexcdevelop.github.io/apidocs/spot_v3_en/#kline-candlestick-data
  # @param symbol [String]
  # @param interval [String]
  # @param start_time [Integer]
  # @param end_time [Integer]
  # @param limit [Integer]
  def candlestick_data(symbol:, interval:, start_time: nil, end_time: nil, limit: 500)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/klines'
        req.headers = headers
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

  # https://mexcdevelop.github.io/apidocs/spot_v3_en/#account-information
  # @param recv_window [Integer]
  def account_information(recv_window: 5000)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/account'
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

  # https://mexcdevelop.github.io/apidocs/spot_v3_en/#query-order
  # @param symbol [String]
  # @param order_id [String]
  # @param orig_client_order_id [String]
  # @param recv_window [Integer]
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

  # https://mexcdevelop.github.io/apidocs/spot_v3_en/#new-order
  # @param symbol [String]
  # @param side [String]
  # @param type [String]
  # @param time_in_force [String]
  # @param quantity [String]
  # @param quote_order_qty [String]
  # @param price [String]
  # @param new_client_order_id [String]
  # @param recv_window [Integer]
  def new_order(
    symbol:,
    side:,
    type:,
    time_in_force: nil,
    quantity: nil,
    quote_order_qty: nil,
    price: nil,
    new_client_order_id: nil,
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
          recvWindow: recv_window,
          timestamp: timestamp
        }.compact
        req.params[:signature] = hmac_signature(req.params)
      end
      Result::Success.new(response.body)
    end
  end

  # https://mexcdevelop.github.io/apidocs/spot_v3_en/#cancel-order
  # @param symbol [String]
  # @param order_id [String]
  # @param orig_client_order_id [String]
  # @param new_client_order_id [String]
  # @param recv_window [Integer]
  def cancel_order(
    symbol:,
    order_id: nil,
    orig_client_order_id: nil,
    new_client_order_id: nil,
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
          recvWindow: recv_window,
          timestamp: timestamp
        }.compact
        req.params[:signature] = hmac_signature(req.params)
      end
      Result::Success.new(response.body)
    end
  end

  # https://mexcdevelop.github.io/apidocs/spot_v3_en/#withdraw
  # @param coin [String] Coin name (e.g., "BTC")
  # @param address [String] Withdrawal address
  # @param amount [String] Withdrawal amount
  # @param network [String] Optional network name
  # @param memo [String] Optional memo/tag
  # @param recv_window [Integer]
  def withdraw(coin:, address:, amount:, network: nil, memo: nil, recv_window: 5000)
    with_rescue do
      response = self.class.connection.post do |req|
        req.url '/api/v3/capital/withdraw/apply'
        req.headers = headers
        req.params = {
          coin: coin,
          address: address,
          amount: amount,
          network: network,
          memo: memo,
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
      Accept: 'application/json',
      'Content-Type': 'application/json'
    }
  end

  def authenticated_headers
    {
      'X-MEXC-APIKEY': @api_key,
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
