class Clients::Kraken < Client
  # https://docs.kraken.com/api/docs/rest-api/add-order
  # https://docs.kraken.com/api/docs/guides/spot-rest-auth#authentication

  URL = 'https://api.kraken.com'.freeze

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

  # https://docs.kraken.com/api/docs/rest-api/get-orders-info
  # @param trades [Boolean] Whether or not to include trades related to position in output
  # @param userref [Integer] Restrict results to given user reference id
  # @param txid [String] The Kraken order identifier. To query multiple orders, use comma delimited list of up to 50 ids.
  # @param consolidate_taker [Boolean] Whether or not to consolidate trades by individual taker trades
  def query_orders_info(txid:, trades: nil, userref: nil, consolidate_taker: true)
    with_rescue do
      response = self.class.connection.post do |req|
        req.url '/0/private/QueryOrders'
        req.body = {
          nonce: nonce,
          trades: trades,
          userref: userref,
          txid: txid,
          consolidate_taker: consolidate_taker
        }.compact.to_query
        req.headers = headers(req.path, req.body)
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.kraken.com/api/docs/rest-api/add-order
  # @param userref [Integer] an optional non-unique, numeric identifier which can associated with a number of
  #                          orders by the client. This field is mutually exclusive with cl_ord_id parameter.
  # @param cl_ord_id [String] an alphanumeric client order identifier which uniquely identifies an open order
  #                           for each client. This field is mutually exclusive with userref parameter.
  # @param ordertype [String] the execution model of the order. Valid values: [market, limit, iceberg,
  #                           stop-loss, take-profit, stop-loss-limit, take-profit-limit, trailing-stop,
  #                           trailing-stop-limit, settle-position]
  # @param type [String] order direction. Valid values: [buy, sell]
  # @param volume [String] order quantity in terms of the base asset.
  # @param displayvol [String] for iceberg orders only, it defines the quantity to show in the book while the
  #                            rest of order quantity remains hidden.
  # @param pair [String] asset pair id or altname.
  # @param price [String] price:
  # • Limit price for limit and iceberg orders
  # • Trigger price for stop-loss, stop-loss-limit, take-profit, take-profit-limit, trailing-stop and
  #   trailing-stop-limit orders
  # @param price2 [String] secondary price:
  # • Limit price for stop-loss-limit, take-profit-limit and trailing-stop-limit orders
  # @param trigger [String] possible values: [index, last]
  # @param leverage [String] the amount of leverage desired (default: none).
  # @param reduce_only [Boolean] if true, order will only reduce a currently open position, not increase it or
  #                              open a new position.
  # @param stptype [String] the self trade prevention (STP) mode. Valid values: [cancel-newest, cancel-oldest,
  #                         cancel-both]
  # @param oflags [Array<String>] list of order flags. Valid values: [post, fcib, fciq, nompp, viqc]
  # @param timeinforce [String] the time-in-force of the order. Valid values: [GTC, IOC, GTD]
  # @param starttm [String] scheduled start time.
  # @param expiretm [String] expiry time.
  # @param close [String] conditional close order type. Valid values: [limit, iceberg, stop-loss, take-profit,
  #                       stop-loss-limit, take-profit-limit, trailing-stop, trailing-stop-limit]
  # @param close_price [String] conditional close order price.
  # @param close_price2 [String] conditional close order price2.
  # @param deadline [String] RFC3339 timestamp (e.g. 2021-04-01T00:18:45Z) after which the matching engine
  #                          should reject the new order request.
  # @param validate [Boolean] if set to true the order will be validated only, it will not trade in the
  #                           matching engine.
  def add_order(
    ordertype:,
    type:,
    volume:,
    pair:,
    userref: nil,
    cl_ord_id: nil,
    displayvol: nil,
    price: nil,
    price2: nil,
    trigger: nil,
    leverage: nil,
    reduce_only: nil,
    stptype: nil,
    oflags: [],
    timeinforce: nil,
    starttm: nil,
    expiretm: nil,
    close: nil,
    close_price: nil,
    close_price2: nil,
    deadline: nil,
    validate: nil
  )
    with_rescue do # rubocop:disable Metrics/BlockLength
      response = self.class.connection.post do |req| # rubocop:disable Metrics/BlockLength
        req.url '/0/private/AddOrder'
        req.body = {
          'nonce' => nonce,
          'ordertype' => ordertype,
          'type' => type,
          'volume' => volume,
          'pair' => pair,
          'userref' => userref,
          'cl_ord_id' => cl_ord_id,
          'displayvol' => displayvol,
          'price' => price,
          'price2' => price2,
          'trigger' => trigger,
          'leverage' => leverage,
          'reduce_only' => reduce_only,
          'stptype' => stptype,
          'oflags' => oflags.any? ? oflags.join(',') : nil,
          'timeinforce' => timeinforce,
          'starttm' => starttm,
          'expiretm' => expiretm,
          'close[ordertype]' => close,
          'close[price]' => close_price,
          'close[price2]' => close_price2,
          'deadline' => deadline,
          'validate' => validate
        }.compact.to_query
        req.headers = headers(req.path, req.body)
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.kraken.com/api/docs/rest-api/cancel-order
  # @param txid [String] The Kraken order identifier.
  # @param cl_ord_id [String] An alphanumeric client order identifier which uniquely identifies an open order for each client.
  def cancel_order(txid: nil, cl_ord_id: nil)
    with_rescue do
      response = self.class.connection.post do |req|
        req.url '/0/private/CancelOrder'
        req.body = {
          nonce: nonce,
          txid: txid,
          cl_ord_id: cl_ord_id
        }.compact.to_query
        req.headers = headers(req.path, req.body)
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.kraken.com/api/docs/rest-api/get-tradable-asset-pairs
  # @param pairs [Array<String>] Asset pairs to get data for
  # @param info [String] Possible values: [info, leverage, fees, margin]
  # @param country_code [String] Filter for response to only include pairs available in provided countries/regions
  def get_tradable_asset_pairs(
    pairs: nil,
    info: nil,
    country_code: nil
  )
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/0/public/AssetPairs'
        req.headers = headers(req.path, req.body)
        req.params = {
          pair: pairs.present? ? pairs.join(',') : nil,
          info: info,
          country_code: country_code
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.kraken.com/api/docs/rest-api/get-asset-info
  # @param assets [Array<String>] Comma delimited list of assets to get info on (optional, default all available assets)
  # @param aclass [String] Asset class (optional, default: currency)
  def get_asset_info(
    assets: nil,
    aclass: nil
  )
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/0/public/Assets'
        req.headers = headers(req.path, req.body)
        req.params = {
          asset: assets.present? ? assets.join(',') : nil,
          aclass: aclass
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.kraken.com/api/docs/rest-api/get-ticker-information
  # @param pair [String] Asset pair to get data for
  def get_ticker_information(pair: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/0/public/Ticker'
        req.headers = headers(req.path, req.body)
        req.params = {
          pair: pair
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.kraken.com/api/docs/rest-api/get-extended-balance
  def get_extended_balance
    with_rescue do
      response = self.class.connection.post do |req|
        req.url '/0/private/BalanceEx'
        req.body = {
          nonce: nonce
        }.to_query
        req.headers = headers(req.path, req.body)
      end
      Result::Success.new(response.body)
    end
  end

  # https://docs.kraken.com/api/docs/rest-api/get-ohlc-data
  # @param pair [String] Asset pair to get data for
  # @param interval [Integer] Time frame interval in minutes
  # @param since [Integer] Return OHLC entries since the given timestamp (intended for incremental updates)
  def get_ohlc_data(pair:, interval: nil, since: nil)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url '/0/public/OHLC'
        req.headers = headers(req.path, req.body)
        req.params = {
          pair: pair,
          interval: interval,
          since: since
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  private

  def nonce
    (Time.now.utc.to_f * 1_000_000).to_i
  end

  def headers(path, body)
    return unauthenticated_headers if unauthenticated? || path.include?('/public/')

    request_nonce = URI.decode_www_form(body).to_h['nonce']
    data = "#{request_nonce}#{body}"
    message = path + Digest::SHA256.digest(data)
    decoded_key = Base64.decode64(@api_secret)
    begin
      hmac = OpenSSL::HMAC.digest('sha512', decoded_key, message)
    rescue OpenSSL::HMACError
      return unauthenticated_headers
    end
    signature = Base64.strict_encode64(hmac)

    {
      'API-Key': @api_key,
      'API-Sign': signature,
      'Accept': 'application/json',
      'Content-Type': 'application/x-www-form-urlencoded',
      'User-Agent': 'Kraken REST API'
    }
  end

  def unauthenticated_headers
    {
      'Accept': 'application/json',
      'Content-Type': 'application/json',
      'User-Agent': 'Kraken REST API'
    }
  end

  def unauthenticated?
    @api_key.blank? || @api_secret.blank?
  end
end
