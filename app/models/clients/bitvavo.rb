class Clients::Bitvavo < Client
  # https://docs.bitvavo.com/

  URL = 'https://api.bitvavo.com'.freeze
  ACCESS_WINDOW = '10000'.freeze

  def initialize(api_key: nil, api_secret: nil)
    super()
    @api_key = api_key
    @api_secret = api_secret
  end

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.proxy = ENV['PROXY_BITVAVO'] if ENV['PROXY_BITVAVO'].present?
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # https://docs.bitvavo.com/#tag/Market-Data/paths/~1v2~1assets/get
  def get_assets
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/v2/assets'
        req.headers = unauthenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.bitvavo.com/#tag/Market-Data/paths/~1v2~1markets/get
  # @param market [String] Filter on a specific market (e.g. "BTC-EUR")
  def markets(market: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/v2/markets'
        req.headers = unauthenticated_headers
        req.params = {
          market: market
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.bitvavo.com/#tag/Market-Data/paths/~1v2~1ticker~1price/get
  # @param market [String]
  def ticker_price(market: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/v2/ticker/price'
        req.headers = unauthenticated_headers
        req.params = {
          market: market
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.bitvavo.com/#tag/Market-Data/paths/~1v2~1ticker~1book/get
  # @param market [String]
  def ticker_book(market: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/v2/ticker/book'
        req.headers = unauthenticated_headers
        req.params = {
          market: market
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.bitvavo.com/#tag/Market-Data/paths/~1v2~1{market}~1candles/get
  # @param market [String]
  # @param interval [String] 1m, 5m, 15m, 30m, 1h, 2h, 4h, 6h, 8h, 12h, 1d
  # @param start_time [Integer] Start time in milliseconds
  # @param end_time [Integer] End time in milliseconds
  # @param limit [Integer]
  def candles(market:, interval:, start_time: nil, end_time: nil, limit: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url "/v2/#{market}/candles"
        req.headers = unauthenticated_headers
        req.params = {
          interval: interval,
          start: start_time,
          end: end_time,
          limit: limit
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.bitvavo.com/#tag/Account/paths/~1v2~1balance/get
  # @param symbol [String]
  def balance(symbol: nil)
    with_rescue do
      path = '/v2/balance'
      params = { symbol: symbol }.compact
      response = self.class.connection.get do |req|
        req.url path
        query_string = params.any? ? "?#{Faraday::Utils.build_query(params)}" : ''
        req.headers = authenticated_headers('GET', "#{path}#{query_string}", '')
        req.params = params
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.bitvavo.com/#tag/Orders/paths/~1v2~1order/get
  # @param market [String]
  # @param order_id [String]
  def get_order(market:, order_id:)
    with_rescue do
      path = '/v2/order'
      params = { market: market, orderId: order_id }
      response = self.class.connection.get do |req|
        req.url path
        query_string = "?#{Faraday::Utils.build_query(params)}"
        req.headers = authenticated_headers('GET', "#{path}#{query_string}", '')
        req.params = params
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.bitvavo.com/#tag/Orders/paths/~1v2~1order/post
  # @param market [String]
  # @param side [String] buy, sell
  # @param order_type [String] market, limit
  # @param amount [String]
  # @param amount_quote [String]
  # @param price [String]
  # @param time_in_force [String]
  def create_order(
    market:,
    side:,
    order_type:,
    amount: nil,
    amount_quote: nil,
    price: nil,
    time_in_force: nil
  )
    with_rescue do
      path = '/v2/order'
      body = {
        market: market,
        side: side,
        orderType: order_type,
        amount: amount,
        amountQuote: amount_quote,
        price: price,
        timeInForce: time_in_force
      }.compact
      body_string = body.to_json
      response = self.class.connection.post do |req|
        req.url path
        req.headers = authenticated_headers('POST', path, body_string)
        req.body = body
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.bitvavo.com/#tag/Orders/paths/~1v2~1order/delete
  # @param market [String]
  # @param order_id [String]
  def cancel_order(market:, order_id:)
    with_rescue do
      path = '/v2/order'
      params = { market: market, orderId: order_id }
      response = self.class.connection.delete do |req|
        req.url path
        query_string = "?#{Faraday::Utils.build_query(params)}"
        req.headers = authenticated_headers('DELETE', "#{path}#{query_string}", '')
        req.params = params
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.bitvavo.com/#tag/Account/paths/~1v2~1withdrawal/post
  # @param symbol [String] Currency symbol (e.g., "BTC")
  # @param amount [String] Withdrawal amount
  # @param address [String] Withdrawal address
  # @param payment_id [String] Optional payment ID / memo / tag
  def withdrawal(symbol:, amount:, address:, payment_id: nil)
    with_rescue do
      path = '/v2/withdrawal'
      body = {
        symbol: symbol,
        amount: amount,
        address: address,
        paymentId: payment_id
      }.compact
      body_string = body.to_json
      response = self.class.connection.post do |req|
        req.url path
        req.headers = authenticated_headers('POST', path, body_string)
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

  def authenticated_headers(method, url_path, body)
    return unauthenticated_headers if unauthenticated?

    ts = timestamp
    payload = "#{ts}#{method}#{url_path}#{body}"
    signature = OpenSSL::HMAC.hexdigest('sha256', @api_secret, payload)

    {
      'BITVAVO-ACCESS-KEY': @api_key,
      'BITVAVO-ACCESS-SIGNATURE': signature,
      'BITVAVO-ACCESS-TIMESTAMP': ts.to_s,
      'BITVAVO-ACCESS-WINDOW': ACCESS_WINDOW,
      Accept: 'application/json',
      'Content-Type': 'application/json'
    }
  end

  def timestamp
    (Time.now.utc.to_f * 1_000).to_i
  end
end
