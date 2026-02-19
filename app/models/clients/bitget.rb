class Clients::Bitget < Client
  # https://www.bitget.com/api-doc/spot/account/Get-Account-Assets

  URL = 'https://api.bitget.com'.freeze

  def initialize(api_key: nil, api_secret: nil, passphrase: nil)
    super()
    @api_key = api_key
    @api_secret = api_secret
    @passphrase = passphrase
  end

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.proxy = ENV['PROXY_BITGET'] if ENV['PROXY_BITGET'].present?
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # https://www.bitget.com/api-doc/spot/public/get-coins
  def get_coins
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v2/spot/public/coins'
        req.headers = unauthenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.bitget.com/api-doc/spot/market/Get-Symbols
  def get_symbols
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v2/spot/public/symbols'
        req.headers = unauthenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.bitget.com/api-doc/spot/market/Get-Tickers
  # @param symbol [String] optional trading pair
  def get_tickers(symbol: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v2/spot/market/tickers'
        req.headers = unauthenticated_headers
        req.params = { symbol: symbol }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.bitget.com/api-doc/spot/market/Get-Orderbook
  # @param symbol [String] trading pair
  # @param limit [Integer] depth limit
  def get_orderbook(symbol:, limit: 20)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v2/spot/market/orderbook'
        req.headers = unauthenticated_headers
        req.params = { symbol: symbol, limit: limit }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.bitget.com/api-doc/spot/market/Get-Candle-Data
  # @param symbol [String] trading pair
  # @param granularity [String] candle interval
  # @param start_time [String] start time in milliseconds
  # @param end_time [String] end time in milliseconds
  # @param limit [Integer] max number of candles
  def get_candles(symbol:, granularity:, start_time: nil, end_time: nil, limit: 1000)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v2/spot/market/candles'
        req.headers = unauthenticated_headers
        req.params = {
          symbol: symbol,
          granularity: granularity,
          startTime: start_time,
          endTime: end_time,
          limit: limit
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.bitget.com/api-doc/spot/account/Get-Account-Assets
  def get_assets
    with_rescue do
      path = '/api/v2/spot/account/assets'
      response = self.class.connection.get do |req|
        req.url path
        req.headers = authenticated_headers('GET', path)
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.bitget.com/api-doc/spot/trade/Get-Order-Info
  # @param order_id [String] order ID
  def get_order(order_id:)
    with_rescue do
      path = '/api/v2/spot/trade/orderInfo'
      query = "orderId=#{order_id}"
      response = self.class.connection.get do |req|
        req.url path
        req.params = { orderId: order_id }
        req.headers = authenticated_headers('GET', path, query: query)
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.bitget.com/api-doc/spot/trade/Place-Order
  # @param symbol [String] trading pair
  # @param side [String] buy or sell
  # @param order_type [String] market or limit
  # @param force [String] time in force (gtc, ioc, fok)
  # @param price [String] price for limit orders
  # @param size [String] base amount
  # @param quote_size [String] quote amount for market buy
  def place_order(symbol:, side:, order_type:, force:, price: nil, size: nil, quote_size: nil)
    with_rescue do
      path = '/api/v2/spot/trade/place-order'
      body = {
        symbol: symbol,
        side: side,
        orderType: order_type,
        force: force,
        price: price,
        size: size,
        quoteSize: quote_size
      }.compact
      response = self.class.connection.post do |req|
        req.url path
        req.headers = authenticated_headers('POST', path, body: body.to_json)
        req.body = body
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.bitget.com/api-doc/spot/trade/Cancel-Order
  # @param order_id [String] order ID
  def cancel_order(order_id:)
    with_rescue do
      path = '/api/v2/spot/trade/cancel-order'
      body = { orderId: order_id }
      response = self.class.connection.post do |req|
        req.url path
        req.headers = authenticated_headers('POST', path, body: body.to_json)
        req.body = body
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.bitget.com/api-doc/spot/account/Wallet-Withdrawal
  # @param coin [String] Coin name (e.g., "BTC")
  # @param transfer_type [String] on_chain
  # @param address [String] Withdrawal address
  # @param size [String] Withdrawal amount
  # @param chain [String] Chain name (e.g., "BTC")
  # @param tag [String] Optional tag/memo
  def withdraw(coin:, address:, size:, chain:, transfer_type: 'on_chain', tag: nil)
    with_rescue do
      path = '/api/v2/spot/wallet/withdrawal'
      body = {
        coin: coin,
        transferType: transfer_type,
        address: address,
        size: size,
        chain: chain,
        tag: tag
      }.compact
      response = self.class.connection.post do |req|
        req.url path
        req.headers = authenticated_headers('POST', path, body: body.to_json)
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
    @api_key.blank? || @api_secret.blank? || @passphrase.blank?
  end

  def authenticated_headers(method, path, body: '', query: nil)
    return unauthenticated_headers if unauthenticated?

    ts = timestamp
    request_path = query.present? ? "#{path}?#{query}" : path
    sign_string = "#{ts}#{method.upcase}#{request_path}#{body}"
    signature = Base64.strict_encode64(
      OpenSSL::HMAC.digest('sha256', @api_secret, sign_string)
    )

    {
      'ACCESS-KEY': @api_key,
      'ACCESS-SIGN': signature,
      'ACCESS-TIMESTAMP': ts,
      'ACCESS-PASSPHRASE': @passphrase,
      Accept: 'application/json',
      'Content-Type': 'application/json'
    }
  end

  def timestamp
    (Time.now.utc.to_f * 1_000).to_i.to_s
  end
end
