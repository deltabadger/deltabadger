class CoinbaseClient < ApplicationClient
  # https://docs.cdp.coinbase.com/coinbase-app/trade/docs/api-overview
  URL = 'https://api.coinbase.com'.freeze

  def initialize(api_key: nil, api_secret: nil)
    super()
    @api_key = api_key
    @api_secret = api_secret&.gsub('\n', "\n")
  end

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.proxy = "https://#{ENV['US_HTTPS_PROXY']}" if ENV['US_HTTPS_PROXY'].present?
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # https://docs.cdp.coinbase.com/coinbase-app/trade/reference/retailbrokerageapi_gethistoricalorder
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

  # https://docs.cdp.coinbase.com/coinbase-app/trade/reference/retailbrokerageapi_postorder
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

  # https://docs.cdp.coinbase.com/coinbase-app/trade/reference/retailbrokerageapi_getpublicproducts
  # @param limit [Integer] The number of products to return
  # @param offset [Integer] The offset for pagination
  # @param product_type [String] The product type (spot or future)
  # @param product_ids [Array<String>] The product ids
  # @param contract_expiry_type [String] The contract expiry type (expiring or perpetual)
  # @param expiring_contract_status [String] The expiring contract status (status_unexpired, status_expired, status_all)
  # @param get_tradability_status [Boolean] Whether to get the tradability status
  # @param get_all_products [Boolean] Whether to get all products
  def list_products(
    limit: nil,
    offset: nil,
    product_type: nil,
    product_ids: nil,
    contract_expiry_type: nil,
    expiring_contract_status: nil,
    get_tradability_status: nil,
    get_all_products: nil
  )
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/brokerage/market/products'
        req.headers = headers(req)
        req.params = {
          limit: limit,
          offset: offset,
          product_type: product_type,
          product_ids: product_ids,
          contract_expiry_type: contract_expiry_type,
          expiring_contract_status: expiring_contract_status,
          get_tradability_status: get_tradability_status,
          get_all_products: get_all_products
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.cdp.coinbase.com/coinbase-app/trade/reference/retailbrokerageapi_getpublicproduct
  # @param product_id [String] The product id
  # @param get_tradability_status [Boolean] Whether or not to populate view_only with the tradability status of the product. This is only enabled for SPOT products. # rubocop:disable Layout/LineLength
  def get_product(
    product_id:,
    get_tradability_status: nil
  )
    with_rescue do
      response = self.class.connection.get do |req|
        req.url "/api/v3/brokerage/market/products/#{product_id}"
        req.headers = headers(req)
        req.params = {
          get_tradability_status: get_tradability_status
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.cdp.coinbase.com/coinbase-app/trade/reference/retailbrokerageapi_getpublicproductbook
  # @param product_id [String] The product id
  # @param limit [Integer] The number of bid/asks to be returned
  # @param aggregation_price_increment [String] The minimum price intervals at which buy and sell orders are grouped or combined in the order book # rubocop:disable Layout/LineLength
  def get_public_product_book(
    product_id:,
    limit: nil,
    aggregation_price_increment: nil
  )
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/brokerage/market/product_book'
        req.headers = headers(req)
        req.params = {
          product_id: product_id,
          limit: limit,
          aggregation_price_increment: aggregation_price_increment
        }.compact
        req.options.params_encoder = Faraday::FlatParamsEncoder
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.cdp.coinbase.com/coinbase-app/trade/reference/retailbrokerageapi_getpubliccandles
  # @param product_id [String] The trading pair (e.g. 'BTC-USD')
  # @param start_time [String] The UNIX timestamp indicating the start of the time interval
  # @param end_time [String] The UNIX timestamp indicating the end of the time interval
  # @param granularity [String] The timeframe each candle represents. Can be: 'ONE_MINUTE', 'FIVE_MINUTES', 'FIFTEEN_MINUTES', 'THIRTY_MINUTES', 'ONE_HOUR', 'TWO_HOUR', 'SIX_HOURS', 'ONE_DAY'
  # @param limit [Integer] The number of candle buckets to be returned. By default, returns 350 (max 350)
  def get_public_product_candles(
    product_id:,
    start_time:,
    end_time:,
    granularity:,
    limit: nil
  )
    with_rescue do
      response = self.class.connection.get do |req|
        req.url "/api/v3/brokerage/market/products/#{product_id}/candles"
        req.headers = headers(req)
        req.params = {
          start: start_time,
          end: end_time,
          granularity: granularity,
          limit: limit
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.cdp.coinbase.com/coinbase-app/trade/reference/retailbrokerageapi_getapikeypermissions
  def get_api_key_permissions
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/brokerage/key_permissions'
        req.headers = headers(req)
      end
      Result::Success.new(response.body)
    end
  end

  def list_accounts
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/brokerage/accounts'
        req.headers = headers(req)
      end
      Result::Success.new(response.body)
    end
  end

  def list_portfolios
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/brokerage/portfolios'
        req.headers = headers(req)
      end
      Result::Success.new(response.body)
    end
  end

  def get_portfolio_breakdown(
    portfolio_uuid:,
    currency: nil
  )
    with_rescue do
      response = self.class.connection.get do |req|
        req.url "/api/v3/brokerage/portfolios/#{portfolio_uuid}"
        req.headers = headers(req)
        req.params = {
          currency: currency
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  private

  def headers(req)
    if unauthenticated?
      unauthenticated_headers
    else
      authenticated_headers(req)
    end
  end

  def unauthenticated?
    @api_key.blank? || @api_secret.blank?
  end

  def unauthenticated_headers
    {
      'Accept': 'application/json',
      'Content-Type': 'application/json'
    }
  end

  def authenticated_headers(req)
    timestamp = Time.now.utc.to_i
    method = req.http_method.to_s.upcase
    request_host = URI(URL).host
    request_path = req.path

    jwt_payload = {
      sub: @api_key,
      iss: 'coinbase-cloud',
      nbf: timestamp,
      exp: timestamp + 120,
      uri: "#{method} #{request_host}#{request_path}"
    }

    signing_key = ecdsa_key? ? ecdsa_signing_key : ed25519_signing_key
    return unauthenticated_headers if signing_key.nil?

    algorithm = ecdsa_key? ? 'ES256' : 'EdDSA'
    jwt = JWT.encode(jwt_payload, signing_key, algorithm, { kid: @api_key, nonce: SecureRandom.hex })

    {
      'Authorization': "Bearer #{jwt}",
      'Accept': 'application/json',
      'Content-Type': 'application/json'
    }
  end

  def ecdsa_key?
    @api_secret.start_with?('-----BEGIN EC PRIVATE KEY-----')
  end

  def ecdsa_signing_key
    OpenSSL::PKey::EC.new(@api_secret)
  rescue OpenSSL::PKey::ECError
    nil
  end

  def ed25519_signing_key
    decoded_key = Base64.decode64(@api_secret)
    seed = decoded_key[0...32]
    RbNaCl::Signatures::Ed25519::SigningKey.new(seed)
  rescue RbNaCl::LengthError
    nil
  end
end
