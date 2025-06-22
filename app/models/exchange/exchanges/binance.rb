module Exchange::Exchanges::Binance
  extend ActiveSupport::Concern

  COINGECKO_ID = 'binance'.freeze # https://docs.coingecko.com/reference/exchanges-list
  ERRORS = {
    insufficient_funds: ['Account has insufficient balance for requested action.'],
    invalid_key: [-2014, -2015]
  }.freeze # https://developers.binance.com/docs/binance-spot-api-docs/errors

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

      error = parse_error_message(result)
      return Result::Failure.new(error) if error.present?

      result.data['symbols'].map do |product|
        ticker = Utilities::Hash.dig_or_raise(product, 'symbol')

        filters = Utilities::Hash.dig_or_raise(product, 'filters')
        price_filter = filters.find { |filter| filter['filterType'] == 'PRICE_FILTER' }
        lot_size_filter = filters.find { |filter| filter['filterType'] == 'LOT_SIZE' }
        notional_filter = filters.find { |filter| filter['filterType'].in?(%w[NOTIONAL MIN_NOTIONAL]) }

        # we use real amount decimals, although Binance allows more precision
        # base_asset_precision = Utilities::Hash.dig_or_raise(product, 'baseAssetPrecision')
        # quote_asset_precision = Utilities::Hash.dig_or_raise(product, 'quoteAssetPrecision')
        # price_increment = Utilities::Hash.dig_or_raise(product, 'pricePrecision')

        {
          ticker: ticker,
          base: Utilities::Hash.dig_or_raise(product, 'baseAsset'),
          quote: Utilities::Hash.dig_or_raise(product, 'quoteAsset'),
          minimum_base_size: lot_size_filter['minQty'].to_d,
          minimum_quote_size: notional_filter['minNotional'].to_d,
          maximum_base_size: lot_size_filter['maxQty'].to_d,
          maximum_quote_size: notional_filter['maxNotional'].to_d,
          base_decimals: Utilities::Number.decimals(lot_size_filter['stepSize']),
          quote_decimals: Utilities::Number.decimals(notional_filter['minNotional']),
          price_decimals: Utilities::Number.decimals(price_filter['tickSize'])
        }
      end.compact
    end

    Result::Success.new(tickers_info)
  end

  def get_tickers_prices(force: false)
    cache_key = "exchange_#{id}_prices"
    tickers_prices = Rails.cache.fetch(cache_key, expires_in: 1.minute, force: force) do
      result = client.symbol_price_ticker
      return Result::Failure.new("Failed to get #{name} products") if result.failure?

      error = parse_error_message(result)
      return Result::Failure.new(error) if error.present?

      result.data.each_with_object({}) do |symbol_price, prices_hash|
        ticker = Utilities::Hash.dig_or_raise(symbol_price, 'symbol')
        price = Utilities::Hash.dig_or_raise(symbol_price, 'price').to_d
        prices_hash[ticker] = price
      end
    end

    Result::Success.new(tickers_prices)
  end

  def get_balances(asset_ids: nil)
    result = client.account_information(omit_zero_balances: true)
    return result if result.failure?

    error = parse_error_message(result)
    return Result::Failure.new(error) if error.present?

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.each_with_object({}) do |asset_id, balances_hash|
      balances_hash[asset_id] = { free: 0, locked: 0 }
    end
    raw_balances = Utilities::Hash.dig_or_raise(result.data, 'balances')
    raw_balances.each do |balance|
      asset = asset_from_symbol(symbol: balance['asset'])
      next unless asset.present?
      next unless asset_ids.include?(asset.id)

      free = Utilities::Hash.dig_or_raise(balance, 'free').to_d
      locked = Utilities::Hash.dig_or_raise(balance, 'locked').to_d

      balances[asset.id] = { free: free, locked: locked }
    end

    Result::Success.new(balances)
  end

  def get_last_price(ticker:, force: false)
    cache_key = "exchange_#{id}_last_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      result = client.symbol_price_ticker(symbol: ticker.ticker)
      return result if result.failure?

      error = parse_error_message(result)
      return Result::Failure.new(error) if error.present?

      price = Utilities::Hash.dig_or_raise(result.data, 'price').to_d
      raise "Wrong last price for #{ticker.ticker}: #{price}" if price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_bid_price(ticker:, force: false)
    cache_key = "exchange_#{id}_bid_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      result = get_bid_ask_price(ticker: ticker)
      return result if result.failure?

      error = parse_error_message(result)
      return Result::Failure.new(error) if error.present?

      price = result.data[:bid][:price]
      raise "Wrong bid price for #{ticker.ticker}: #{price}" if price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_ask_price(ticker:, force: false)
    cache_key = "exchange_#{id}_ask_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      result = get_bid_ask_price(ticker: ticker)
      return result if result.failure?

      price = result.data[:ask][:price]
      raise "Wrong ask price for #{ticker.ticker}: #{price}" if price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_candles(ticker:, start_at:, timeframe:)
    intervals = {
      1.minute => '1m',
      5.minutes => '5m',
      15.minutes => '15m',
      30.minutes => '30m',
      1.hour => '1h',
      4.hours => '4h',
      1.day => '1d',
      3.days => '3d',
      1.week => '1w',
      1.month => '1M'
      # 1.second => '1s',
      # 3.minutes => '3m',
      # 2.hours => '2h',
      # 6.hours => '6h',
      # 8.hours => '8h',
      # 12.hours => '12h',
      # 3.months => '3M',
    }
    interval = intervals[timeframe]

    limit = 1000
    candles = []
    loop do
      result = client.candlestick_data(
        symbol: ticker.ticker,
        start_time: start_at.to_i * 1000,
        interval: interval,
        limit: limit
      )
      return result if result.failure?

      error = parse_error_message(result)
      return Result::Failure.new(error) if error.present?

      result.data.each do |candle|
        candles << [
          Time.at(candle[0] / 1000).utc,
          candle[1].to_d,
          candle[2].to_d,
          candle[3].to_d,
          candle[4].to_d,
          candle[5].to_d
        ]
      end
      break if result.data.last.empty? || result.data.last[0] > timeframe.ago.to_i * 1000

      start_at = candles.empty? ? start_at + limit * interval.to_i * 1000 : candles.last[0] + 1
    end

    Result::Success.new(candles)
  end


  def get_api_key_validity(api_key:) # rubocop:disable Metrics/PerceivedComplexity,Metrics/CyclomaticComplexity
    result = BinanceClient.new(
      api_key: api_key.key,
      api_secret: api_key.secret
    ).api_description

    if result.success?
      valid = if api_key.trading?
                result.data['ipRestrict'] == true &&
                  result.data['enableFixApiTrade'] == false &&
                  result.data['enableFixReadOnly'] == false &&
                  result.data['enableFutures'] == false &&
                  result.data['enableInternalTransfer'] == false &&
                  result.data['enableMargin'] == false &&
                  result.data['enablePortfolioMarginTrading'] == false &&
                  result.data['enableReading'] == true &&
                  result.data['enableSpotAndMarginTrading'] == true &&
                  result.data['enableVanillaOptions'] == false &&
                  result.data['enableWithdrawals'] == false &&
                  result.data['permitsUniversalTransfer'] == false
              elsif api_key.withdrawal?
                result.data['ipRestrict'] == true &&
                  result.data['enableFixApiTrade'] == false &&
                  result.data['enableFixReadOnly'] == false &&
                  result.data['enableFutures'] == false &&
                  result.data['enableInternalTransfer'] == false &&
                  result.data['enableMargin'] == false &&
                  result.data['enablePortfolioMarginTrading'] == false &&
                  result.data['enableReading'] == true &&
                  result.data['enableSpotAndMarginTrading'] == false &&
                  result.data['enableVanillaOptions'] == false &&
                  result.data['enableWithdrawals'] == true &&
                  result.data['permitsUniversalTransfer'] == false
              else
                raise StandardError, 'Invalid API key type'
              end
      Result::Success.new(valid)
    elsif parse_error_code(result).in?(known_errors[:invalid_key])
      Result::Success.new(false)
    else
      result
    end
  end

  private

  def client
    @client ||= set_client
  end

  def parse_error_code(result)
    return unless result.errors.first.present?

    begin
      JSON.parse(result.errors.first)['code']
    rescue StandardError
      nil
    end
  end

  def parse_error_message(result)
    return unless result.errors.first.present?

    begin
      JSON.parse(result.errors.first)['msg']
    rescue StandardError
      nil
    end
  end

  def asset_from_symbol(symbol:)
    @asset_from_symbol ||= tickers.includes(:base_asset, :quote_asset).each_with_object({}) do |t, map|
      map[t.base] ||= t.base_asset
      map[t.quote] ||= t.quote_asset
    end
    @asset_from_symbol[symbol]
  end

  def get_bid_ask_price(ticker:)
    result = client.symbol_order_book_ticker(symbol: ticker.ticker)
    return result if result.failure?

    error = parse_error_message(result)
    return Result::Failure.new(error) if error.present?

    Result::Success.new(
      {
        bid: {
          price: Utilities::Hash.dig_or_raise(result.data, 'bidPrice').to_d,
          size: Utilities::Hash.dig_or_raise(result.data, 'bidQty').to_d
        },
        ask: {
          price: Utilities::Hash.dig_or_raise(result.data, 'askPrice').to_d,
          size: Utilities::Hash.dig_or_raise(result.data, 'askQty').to_d
        }
      }
    )
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
