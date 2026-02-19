class Clients::Kucoin < Client
  # https://www.kucoin.com/docs/rest/spot-trading/spot-hf-trade-pro-account/place-hf-order

  URL = 'https://api.kucoin.com'.freeze

  def initialize(api_key: nil, api_secret: nil, passphrase: nil)
    super()
    @api_key = api_key
    @api_secret = api_secret
    @passphrase = passphrase
  end

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.proxy = ENV['PROXY_KUCOIN'] if ENV['PROXY_KUCOIN'].present?
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # https://www.kucoin.com/docs/rest/funding/funding-overview/get-currency-list
  def get_currencies
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v3/currencies'
        req.headers = unauthenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/spot-trading/market-data/get-symbols-list
  def get_symbols
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v2/symbols'
        req.headers = unauthenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/spot-trading/market-data/get-all-tickers
  def get_all_tickers
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v1/market/allTickers'
        req.headers = unauthenticated_headers
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/spot-trading/market-data/get-part-order-book-aggregated-
  # @param symbol [String] trading pair (e.g., BTC-USDT)
  # @param limit [Integer] 20 or 100
  def get_orderbook(symbol:, limit: 20)
    with_rescue do
      endpoint = "/api/v1/market/orderbook/level2_#{limit}"
      response = self.class.connection.get do |req|
        req.url endpoint
        req.headers = unauthenticated_headers
        req.params = { symbol: symbol }
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/spot-trading/market-data/get-klines
  # @param symbol [String] trading pair
  # @param type [String] candle type (1min, 3min, 5min, 15min, 30min, 1hour, 2hour, 4hour, 6hour, 8hour, 12hour, 1day, 1week)
  # @param start_at [Integer] start time in seconds
  # @param end_at [Integer] end time in seconds
  def get_candles(symbol:, type:, start_at: nil, end_at: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v1/market/candles'
        req.headers = unauthenticated_headers
        req.params = {
          symbol: symbol,
          type: type,
          startAt: start_at,
          endAt: end_at
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/account/basic-info/get-account-list-spot-margin-trade_hf
  # @param type [String] account type (main, trade, margin, trade_hf)
  # @param currency [String] optional currency
  def get_accounts(type: nil, currency: nil)
    with_rescue do
      path = '/api/v1/accounts'
      response = self.class.connection.get do |req|
        req.url path
        req.params = { type: type, currency: currency }.compact
        req.headers = authenticated_headers('GET', path, req.params)
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/spot-trading/orders/get-order-details-by-orderid
  # @param order_id [String] order ID
  def get_order(order_id:)
    with_rescue do
      path = "/api/v1/orders/#{order_id}"
      response = self.class.connection.get do |req|
        req.url path
        req.headers = authenticated_headers('GET', path)
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/spot-trading/orders/place-order
  # @param client_oid [String] unique client order id
  # @param symbol [String] trading pair
  # @param side [String] buy or sell
  # @param type [String] limit or market
  # @param price [String] price for limit orders
  # @param size [String] base amount
  # @param funds [String] quote amount for market buy
  def create_order(client_oid:, symbol:, side:, type:, price: nil, size: nil, funds: nil)
    with_rescue do
      path = '/api/v1/orders'
      body = {
        clientOid: client_oid,
        symbol: symbol,
        side: side,
        type: type,
        price: price,
        size: size,
        funds: funds
      }.compact
      response = self.class.connection.post do |req|
        req.url path
        req.headers = authenticated_headers('POST', path, nil, body.to_json)
        req.body = body
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/spot-trading/orders/cancel-order-by-orderid
  # @param order_id [String] order ID
  def cancel_order(order_id:)
    with_rescue do
      path = "/api/v1/orders/#{order_id}"
      response = self.class.connection.delete do |req|
        req.url path
        req.headers = authenticated_headers('DELETE', path)
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/funding/withdrawals/apply-withdraw-v3-
  # @param currency [String] Currency code (e.g., "BTC")
  # @param address [String] Withdrawal address
  # @param amount [String] Withdrawal amount
  # @param chain [String] Optional chain name
  # @param memo [String] Optional memo/tag
  def withdraw(currency:, address:, amount:, chain: nil, memo: nil)
    with_rescue do
      path = '/api/v3/withdrawals'
      body = {
        currency: currency,
        address: address,
        amount: amount,
        chain: chain,
        memo: memo
      }.compact
      response = self.class.connection.post do |req|
        req.url path
        req.headers = authenticated_headers('POST', path, nil, body.to_json)
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

  def authenticated_headers(method, path, params = nil, body = nil)
    return unauthenticated_headers if unauthenticated?

    ts = timestamp
    endpoint = path
    endpoint = "#{path}?#{Faraday::Utils.build_query(params)}" if params.present?
    sign_string = "#{ts}#{method.upcase}#{endpoint}#{body}"
    signature = Base64.strict_encode64(
      OpenSSL::HMAC.digest('sha256', @api_secret, sign_string)
    )
    # KC-API-KEY-VERSION 2 requires HMAC'd passphrase
    passphrase_sign = Base64.strict_encode64(
      OpenSSL::HMAC.digest('sha256', @api_secret, @passphrase)
    )

    {
      'KC-API-KEY': @api_key,
      'KC-API-SIGN': signature,
      'KC-API-TIMESTAMP': ts,
      'KC-API-PASSPHRASE': passphrase_sign,
      'KC-API-KEY-VERSION': '2',
      Accept: 'application/json',
      'Content-Type': 'application/json'
    }
  end

  def timestamp
    (Time.now.utc.to_f * 1_000).to_i.to_s
  end
end
