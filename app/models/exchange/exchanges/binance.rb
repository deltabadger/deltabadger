module Exchange::Exchanges::Binance
  extend ActiveSupport::Concern

  COINGECKO_ID = 'binance'.freeze # https://docs.coingecko.com/reference/exchanges-list
  TICKER_BLACKLIST = [].freeze
  ERRORS = {
    insufficient_funds: ['Account has insufficient balance for requested action.']
  }.freeze

  include Exchange::Dryable # decorators for: get_order, get_orders, cancel_order, get_api_key_validity, set_market_order, set_limit_order

  attr_reader :api_key

  def coingecko_id
    COINGECKO_ID
  end

  def known_errors
    ERRORS
  end

  def proxy_ip
    @proxy_ip ||= BinanceClient::PROXY.split('://').last.split(':').first if BinanceClient::PROXY.present?
  end

  def set_client(api_key: nil)
    @api_key = api_key
    @client = BinanceClient.new(
      api_key: api_key&.key,
      api_secret: api_key&.secret
    )
  end

  def get_tickers_info(force: false)
    cache_key = "exchange_#{id}_tickers_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour, force: force) do
      result = client.exchange_information(permissions: ['SPOT'])
      return Result::Failure.new("Failed to get #{name} products") if result.failure?

      result.data['symbols'].map do |product|
        ticker = Utilities::Hash.dig_or_raise(product, 'symbol')
        next if TICKER_BLACKLIST.include?(ticker)

        filters = Utilities::Hash.dig_or_raise(product, 'filters')
        tick_size = filters.find { |filter| filter['filterType'] == 'PRICE_FILTER' }['tickSize'].to_d
        min_qty = filters.find { |filter| filter['filterType'] == 'LOT_SIZE' }['minQty'].to_d
        max_qty = filters.find { |filter| filter['filterType'] == 'LOT_SIZE' }['maxQty'].to_d
        step_size = filters.find { |filter| filter['filterType'] == 'LOT_SIZE' }['stepSize'].to_d
        notional_filter = filters.find { |filter| filter['filterType'] == 'NOTIONAL' } ||
                          filters.find { |filter| filter['filterType'] == 'MIN_NOTIONAL' }
        min_notional = notional_filter['minNotional'].to_d
        max_notional = notional_filter['maxNotional'].to_d

        # we use real amount decimals, although Binance allows more precision
        # base_asset_precision = Utilities::Hash.dig_or_raise(product, 'baseAssetPrecision')
        # quote_asset_precision = Utilities::Hash.dig_or_raise(product, 'quoteAssetPrecision')
        # price_increment = Utilities::Hash.dig_or_raise(product, 'pricePrecision')

        {
          ticker: ticker,
          base: Utilities::Hash.dig_or_raise(product, 'baseAsset'),
          quote: Utilities::Hash.dig_or_raise(product, 'quoteAsset'),
          minimum_base_size: min_qty,
          minimum_quote_size: min_notional,
          maximum_base_size: max_qty,
          maximum_quote_size: max_notional,
          base_decimals: Utilities::Number.decimals(step_size),
          quote_decimals: Utilities::Number.decimals(min_notional),
          price_decimals: Utilities::Number.decimals(tick_size)
        }
      end.compact
    end

    Result::Success.new(tickers_info)
  end

  private

  def client
    @client ||= set_client
  end

  def parse_order_status(status)
    # NEW: The order has been accepted by the engine.
    # PENDING_NEW: The order is in a pending phase until the working order of an order list has been fully filled.
    # PARTIALLY_FILLED: A part of the order has been filled.
    # FILLED: The order has been completed.
    # CANCELED: The order has been canceled by the user.
    # PENDING_CANCEL: Currently unused
    # REJECTED: The order was not accepted by the engine and not processed.
    # EXPIRED: The order was canceled according to the order type's rules (e.g. LIMIT FOK orders with no fill,
    #          LIMIT IOC or MARKET orders that partially fill) or by the exchange, (e.g. orders canceled during
    #          liquidation, orders canceled during maintenance)
    # EXPIRED_IN_MATCH: The order was expired by the exchange due to STP. (e.g. an order with EXPIRE_TAKER will
    #                   match with existing orders on the book with the same account or same tradeGroupId)
    case status
    when 'PENDING_CANCEL'
      :unknown
    when 'NEW', 'PENDING_NEW', 'PARTIALLY_FILLED'
      :open
    when 'FILLED', 'CANCELED', 'EXPIRED', 'EXPIRED_IN_MATCH'
      :closed
    when 'REJECTED'
      :failed # Warning! This is not a valid external_status.
    else
      raise "Unknown #{name} order status: #{status}"
    end
  end
end
