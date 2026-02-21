class Clients::Gemini < Client
  # https://docs.gemini.com/rest-api/

  URL = 'https://api.gemini.com'.freeze

  def initialize(api_key: nil, api_secret: nil)
    super()
    @api_key = api_key
    @api_secret = api_secret
  end

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.proxy = ENV['PROXY_GEMINI'] if ENV['PROXY_GEMINI'].present?
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # https://docs.gemini.com/rest-api/#symbols
  def get_symbols
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/v1/symbols'
        req.headers = unauthenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.gemini.com/rest-api/#symbol-details
  # @param symbol [String] trading pair (e.g., btcusd)
  def get_symbol_details(symbol:)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url "/v1/symbols/details/#{symbol}"
        req.headers = unauthenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.gemini.com/rest-api/#price-feed
  def get_price_feed
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/v1/pricefeed'
        req.headers = unauthenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.gemini.com/rest-api/#ticker-v2
  # @param symbol [String] trading pair (e.g., btcusd)
  def get_ticker(symbol:)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url "/v2/ticker/#{symbol}"
        req.headers = unauthenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.gemini.com/rest-api/#new-order
  # @param symbol [String] trading pair
  # @param amount [String] base amount
  # @param price [String] price
  # @param side [String] buy or sell
  # @param type [String] exchange limit
  # @param options [Array<String>] optional order options (e.g., ["immediate-or-cancel"])
  def new_order(symbol:, amount:, price:, side:, type: 'exchange limit', options: [])
    with_rescue do
      path = '/v1/order/new'
      payload = {
        request: path,
        nonce: nonce,
        symbol: symbol,
        amount: amount,
        price: price,
        side: side,
        type: type,
        options: options
      }.compact
      response = self.class.connection.post do |req|
        req.url path
        req.headers = authenticated_headers(payload)
        # Gemini expects no body in the request; everything is in the headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.gemini.com/rest-api/#order-status
  # @param order_id [String] order ID
  def order_status(order_id:)
    with_rescue do
      path = '/v1/order/status'
      payload = {
        request: path,
        nonce: nonce,
        order_id: order_id
      }
      response = self.class.connection.post do |req|
        req.url path
        req.headers = authenticated_headers(payload)
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.gemini.com/rest-api/#cancel-order
  # @param order_id [String] order ID
  def cancel_order(order_id:)
    with_rescue do
      path = '/v1/order/cancel'
      payload = {
        request: path,
        nonce: nonce,
        order_id: order_id
      }
      response = self.class.connection.post do |req|
        req.url path
        req.headers = authenticated_headers(payload)
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.gemini.com/rest-api/#get-available-balances
  def get_balances
    with_rescue do
      path = '/v1/balances'
      payload = {
        request: path,
        nonce: nonce
      }
      response = self.class.connection.post do |req|
        req.url path
        req.headers = authenticated_headers(payload)
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.gemini.com/rest-api/#withdraw-crypto-funds
  # @param currency [String] Currency code (e.g., "BTC")
  # @param address [String] Withdrawal address
  # @param amount [String] Withdrawal amount
  def withdraw_crypto_funds(currency:, address:, amount:)
    with_rescue do
      path = '/v1/withdraw/crypto'
      payload = {
        request: path,
        nonce: nonce,
        currency: currency,
        address: address,
        amount: amount
      }
      response = self.class.connection.post do |req|
        req.url path
        req.headers = authenticated_headers(payload)
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

  def authenticated_headers(payload)
    return unauthenticated_headers if unauthenticated?

    encoded_payload = Base64.strict_encode64(payload.to_json)
    signature = OpenSSL::HMAC.hexdigest('sha384', @api_secret, encoded_payload)

    {
      'X-GEMINI-APIKEY': @api_key,
      'X-GEMINI-PAYLOAD': encoded_payload,
      'X-GEMINI-SIGNATURE': signature,
      Accept: 'application/json',
      'Content-Type': 'text/plain'
    }
  end

  def nonce
    (Time.now.utc.to_f * 1_000_000).to_i.to_s
  end
end
