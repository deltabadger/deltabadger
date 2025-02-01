class CoinbaseClient < ApplicationClient
  # https://docs.cdp.coinbase.com/advanced-trade/docs/api-overview
  URL = 'https://api.coinbase.com'.freeze

  def initialize(api_key: nil, api_secret: nil)
    super()
    @api_key = api_key
    @api_secret = api_secret
  end

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # https://docs.cdp.coinbase.com/advanced-trade/reference/retailbrokerageapi_gethistoricalorder
  # @param order_id [String] The order id
  def get_order(order_id:)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url "/api/v3/brokerage/orders/historical/#{order_id}"
        req.headers = headers(req)
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.cdp.coinbase.com/advanced-trade/reference/retailbrokerageapi_postorder
  # @param client_order_id [String] The client order id
  # @param product_id [String] The product id (BTC-USD)
  # @param side [String] The side of the order (buy or sell)
  # @param order_configuration [Hash] The order configuration
  def create_order(
    client_order_id:,
    product_id:,
    side:,
    order_configuration:
  )
    with_rescue do
      response = self.class.connection.post do |req|
        req.url '/api/v3/brokerage/orders'
        req.headers = headers(req)
        req.body = {
          client_order_id: client_order_id,
          product_id: product_id,
          side: side,
          order_configuration: order_configuration
        }
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.cdp.coinbase.com/advanced-trade/reference/retailbrokerageapi_gettransactionsummary
  # @param product_type [String] The product type (spot or future)
  # @param contract_expiry_type [String] The contract expiry type (expiring or perpetual)
  # @param product_venue [String] The product venue (cbe, fcm, intx)
  def get_transaction_summary(
    product_type: nil,
    contract_expiry_type: nil,
    product_venue: nil
  )
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/brokerage/transaction_summary'
        req.headers = headers(req)
        req.params = {
          product_type: product_type,
          contract_expiry_type: contract_expiry_type,
          product_venue: product_venue
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  private

  def headers(req)
    if cdp_keys?
      cdp_headers(req)
    else
      legacy_headers(req)
    end
  end

  def legacy_headers(req)
    timestamp = Time.now.utc.to_i.to_s
    body = req.body.present? ? req.body : ''
    payload = "#{timestamp}#{req.http_method.to_s.upcase}#{req.path}#{body}"
    signature = OpenSSL::HMAC.hexdigest('sha256', @api_secret, payload)
    {
      'CB-ACCESS-KEY': @api_key,
      'CB-ACCESS-TIMESTAMP': timestamp,
      'CB-ACCESS-SIGN': signature,
      'Accept': 'application/json',
      'Content-Type': 'application/json'
    }
  end

  def cdp_headers(req)
    private_key = OpenSSL::PKey::EC.new(@api_secret)
    uri = "#{req.http_method.to_s.upcase} #{URL}#{req.path}"
    jwt_payload = {
      sub: @api_key,
      iss: 'coinbase-cloud',
      nbf: Time.now.utc.to_i,
      exp: Time.now.utc.to_i + 120,
      uri: uri
    }
    jwt = JWT.encode(jwt_payload, private_key, 'ES256', { kid: @api_key, nonce: SecureRandom.hex })
    {
      'Authorization': "Bearer #{jwt}"
    }
  end

  def cdp_keys?
    @api_secret.start_with?('-----BEGIN EC PRIVATE KEY-----')
  end
end
