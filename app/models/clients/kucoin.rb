class Clients::Kucoin < Client
  # https://www.kucoin.com/docs/rest/spot-trading/orders/place-order

  URL = 'https://api.kucoin.com'.freeze
  PROXY = ENV['EU_HTTPS_PROXY']

  def initialize(api_key: nil, api_secret: nil, passphrase: nil)
    super()
    @api_key = api_key
    @api_secret = api_secret
    @passphrase = passphrase
  end

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.proxy = PROXY
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # https://www.kucoin.com/docs/rest/spot-trading/market-data/get-symbols-list
  def get_symbols(market: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v2/symbols'
        req.headers = headers(req)
        req.params = {
          market:
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/spot-trading/market-data/get-ticker
  # @param symbol [String] Symbol (e.g. 'BTC-USDT')
  def get_ticker(symbol:)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v1/market/orderbook/level1'
        req.headers = headers(req)
        req.params = {
          symbol:
        }
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/spot-trading/market-data/get-all-tickers
  def get_all_tickers
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v1/market/allTickers'
        req.headers = headers(req)
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/spot-trading/market-data/get-part-order-book-aggregated
  # @param symbol [String] Symbol (e.g. 'BTC-USDT')
  def get_order_book(symbol:)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v1/market/orderbook/level1'
        req.headers = headers(req)
        req.params = {
          symbol:
        }
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/spot-trading/market-data/get-klines
  # @param symbol [String] Symbol (e.g. 'BTC-USDT')
  # @param type [String] Type of candlestick patterns: 1min, 3min, 5min, 15min, 30min, 1hour, 2hour, 4hour, 6hour, 8hour, 12hour, 1day, 1week
  # @param start_at [Integer] Start time (Unix timestamp in seconds)
  # @param end_at [Integer] End time (Unix timestamp in seconds)
  def get_klines(symbol:, type:, start_at: nil, end_at: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v1/market/candles'
        req.headers = headers(req)
        req.params = {
          symbol:,
          type:,
          startAt: start_at,
          endAt: end_at
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/account/basic-info/get-account-list-spot-margin-trade_hf
  # @param currency [String] Currency (optional)
  # @param type [String] Account type: main, trade, margin, or trade_hf
  def get_accounts(currency: nil, type: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v1/accounts'
        req.headers = headers(req)
        req.params = {
          currency:,
          type:
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/spot-trading/orders/place-order
  # @param client_oid [String] Unique order id created by users to identify their orders
  # @param side [String] buy or sell
  # @param symbol [String] Symbol (e.g. 'BTC-USDT')
  # @param type [String] Order type: limit or market (default is limit)
  # @param price [String] Price per base currency (required for limit orders)
  # @param size [String] Amount of base currency to buy or sell
  # @param funds [String] Amount of quote currency to use (for market buy orders)
  # @param time_in_force [String] GTC, GTT, IOC, or FOK (default is GTC)
  # @param cancel_after [Integer] Cancel after n seconds
  # @param post_only [Boolean] Post only flag (default is false)
  # @param hidden [Boolean] Hidden order flag (default is false)
  # @param iceberg [Boolean] Iceberg order flag (default is false)
  # @param visible_size [String] The maximum visible size of an iceberg order
  def create_order(
    client_oid:,
    side:,
    symbol:,
    type: 'limit',
    price: nil,
    size: nil,
    funds: nil,
    time_in_force: nil,
    cancel_after: nil,
    post_only: nil,
    hidden: nil,
    iceberg: nil,
    visible_size: nil
  )
    with_rescue do
      response = self.class.connection.post do |req|
        req.url '/api/v1/orders'
        req.headers = headers(req)
        req.body = {
          clientOid: client_oid,
          side:,
          symbol:,
          type:,
          price:,
          size:,
          funds:,
          timeInForce: time_in_force,
          cancelAfter: cancel_after,
          postOnly: post_only,
          hidden:,
          iceberg:,
          visibleSize: visible_size
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/spot-trading/orders/get-order-details-by-orderid
  # @param order_id [String] Order ID
  def get_order(order_id:)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url "/api/v1/orders/#{order_id}"
        req.headers = headers(req)
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/spot-trading/orders/get-order-list
  # @param status [String] active or done
  # @param symbol [String] Symbol (optional)
  # @param side [String] buy or sell (optional)
  # @param type [String] limit or market (optional)
  # @param start_at [Integer] Start time (Unix timestamp in milliseconds)
  # @param end_at [Integer] End time (Unix timestamp in milliseconds)
  # @param current_page [Integer] Current page number (default is 1)
  # @param page_size [Integer] Number of results per page (default is 50, max is 500)
  def get_orders(
    status: nil,
    symbol: nil,
    side: nil,
    type: nil,
    start_at: nil,
    end_at: nil,
    current_page: 1,
    page_size: 500
  )
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v1/orders'
        req.headers = headers(req)
        req.params = {
          status:,
          symbol:,
          side:,
          type:,
          startAt: start_at,
          endAt: end_at,
          currentPage: current_page,
          pageSize: page_size
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/spot-trading/orders/cancel-order-by-orderid
  # @param order_id [String] Order ID
  def cancel_order(order_id:)
    with_rescue do
      response = self.class.connection.delete do |req|
        req.url "/api/v1/orders/#{order_id}"
        req.headers = headers(req)
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/account/sub-account/get-sub-account-spot-api-list
  # This can be used to verify API key validity
  # @param api_key [String] API key (optional)
  # @param sub_name [String] Sub-account name (optional)
  def get_sub_account_api_list(api_key: nil, sub_name: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/api/v1/sub/api-key'
        req.headers = headers(req)
        req.params = {
          apiKey: api_key,
          subName: sub_name
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://www.kucoin.com/docs/rest/account/basic-info/get-account-detail-spot-margin-trade_hf
  # @param account_id [String] Account ID
  def get_account(account_id:)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url "/api/v1/accounts/#{account_id}"
        req.headers = headers(req)
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
    @api_key.blank? || @api_secret.blank? || @passphrase.blank?
  end

  def unauthenticated_headers
    {
      'Accept': 'application/json',
      'Content-Type': 'application/json'
    }
  end

  def authenticated_headers(req)
    timestamp = (Time.now.utc.to_f * 1_000).to_i
    method = req.http_method.to_s.upcase
    request_path = req.path

    # Build the string to sign
    body = if req.body.is_a?(String)
             req.body
           else
             (req.body.present? ? req.body.to_json : '')
           end
    str_to_sign = "#{timestamp}#{method}#{request_path}#{body}"

    # Generate signature
    signature = Base64.strict_encode64(
      OpenSSL::HMAC.digest('sha256', @api_secret, str_to_sign)
    )

    # Sign the passphrase
    passphrase_signature = Base64.strict_encode64(
      OpenSSL::HMAC.digest('sha256', @api_secret, @passphrase)
    )

    {
      'KC-API-KEY': @api_key,
      'KC-API-SIGN': signature,
      'KC-API-TIMESTAMP': timestamp.to_s,
      'KC-API-PASSPHRASE': passphrase_signature,
      'KC-API-KEY-VERSION': '2',
      'Accept': 'application/json',
      'Content-Type': 'application/json'
    }
  end
end
