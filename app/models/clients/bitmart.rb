class Clients::Bitmart < Client
  # https://developer-pro.bitmart.com/en/spot/

  URL = 'https://api-cloud.bitmart.com'.freeze

  def initialize(api_key: nil, api_secret: nil, memo: nil)
    super()
    @api_key = api_key
    @api_secret = api_secret
    @memo = memo
  end

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.proxy = ENV['PROXY_BITMART'] if ENV['PROXY_BITMART'].present?
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # https://developer-pro.bitmart.com/en/spot/#get-symbols-details-v1
  def get_symbols
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/spot/v1/symbols/details'
        req.headers = unauthenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://developer-pro.bitmart.com/en/spot/#get-ticker-of-all-pairs-v3
  # @param symbol [String] optional trading pair
  def get_ticker(symbol: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/spot/quotation/v3/ticker'
        req.headers = unauthenticated_headers
        req.params = { symbol: symbol }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://developer-pro.bitmart.com/en/spot/#get-depth-v3
  # @param symbol [String] trading pair
  # @param limit [Integer] depth limit
  def get_depth(symbol:, limit: 20)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/spot/quotation/v3/books'
        req.headers = unauthenticated_headers
        req.params = { symbol: symbol, limit: limit }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://developer-pro.bitmart.com/en/spot/#get-k-line-v3
  # @param symbol [String] trading pair
  # @param step [Integer] kline step in minutes
  # @param before [Integer] start time in seconds
  # @param after [Integer] end time in seconds
  # @param limit [Integer] max number of candles
  def get_klines(symbol:, step:, before: nil, after: nil, limit: 500)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/spot/quotation/v3/lite-klines'
        req.headers = unauthenticated_headers
        req.params = {
          symbol: symbol,
          step: step,
          before: before,
          after: after,
          limit: limit
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://developer-pro.bitmart.com/en/spot/#get-wallet-balance-keyed
  def get_wallet
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/account/v1/wallet'
        req.headers = authenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://developer-pro.bitmart.com/en/spot/#new-order-v2-signed
  # @param symbol [String] trading pair
  # @param side [String] buy or sell
  # @param type [String] market or limit
  # @param size [String] base amount
  # @param notional [String] quote amount for market buy
  # @param price [String] price for limit orders
  def create_order(symbol:, side:, type:, size: nil, notional: nil, price: nil)
    with_rescue do
      body = {
        symbol: symbol,
        side: side,
        type: type,
        size: size,
        notional: notional,
        price: price
      }.compact
      response = self.class.connection.post do |req|
        req.url '/spot/v2/submit_order'
        req.headers = authenticated_headers(body: body.to_json)
        req.body = body
      end
      Result::Success.new(response.body)
    end
  end

  # https://developer-pro.bitmart.com/en/spot/#query-order-by-id-v4-signed
  # @param order_id [String] order ID
  def get_order(order_id:)
    with_rescue do
      response = self.class.connection.post do |req|
        body = { orderId: order_id }
        req.url '/spot/v4/query/order'
        req.headers = authenticated_headers(body: body.to_json)
        req.body = body
      end
      Result::Success.new(response.body)
    end
  end

  # https://developer-pro.bitmart.com/en/spot/#cancel-order-v3-signed
  # @param symbol [String] trading pair
  # @param order_id [String] order ID
  def cancel_order(symbol:, order_id:)
    with_rescue do
      body = { symbol: symbol, order_id: order_id }
      response = self.class.connection.post do |req|
        req.url '/spot/v3/cancel_order'
        req.headers = authenticated_headers(body: body.to_json)
        req.body = body
      end
      Result::Success.new(response.body)
    end
  end

  # https://developer-pro.bitmart.com/en/spot/#get-currencies
  def get_currencies
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/account/v1/currencies'
        req.headers = unauthenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://developer-pro.bitmart.com/en/spot/#withdraw-signed
  # @param currency [String] Coin name (e.g., "BTC")
  # @param amount [String] Withdrawal amount
  # @param address [String] Withdrawal address
  # @param network [String] Optional network name
  # @param address_memo [String] Optional memo/tag
  # @param destination [String] Destination type (default: "To Address")
  def withdraw(currency:, amount:, address:, network: nil, address_memo: nil, destination: 'To Address')
    with_rescue do
      body = {
        currency: currency,
        amount: amount,
        destination: destination,
        address: address,
        address_memo: address_memo,
        network: network
      }.compact
      response = self.class.connection.post do |req|
        req.url '/account/v1/withdraw/apply'
        req.headers = authenticated_headers(body: body.to_json)
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
    @api_key.blank? || @api_secret.blank? || @memo.blank?
  end

  def authenticated_headers(body: '')
    return unauthenticated_headers if unauthenticated?

    ts = timestamp
    sign_string = "#{ts}##{@memo}##{body}"
    signature = OpenSSL::HMAC.hexdigest('sha256', @api_secret, sign_string)

    {
      'X-BM-KEY': @api_key,
      'X-BM-SIGN': signature,
      'X-BM-TIMESTAMP': ts.to_s,
      Accept: 'application/json',
      'Content-Type': 'application/json'
    }
  end

  def timestamp
    (Time.now.utc.to_f * 1_000).to_i
  end
end
