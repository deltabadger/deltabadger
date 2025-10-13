class Exchanges::Kucoin < Exchange
  COINGECKO_ID = 'kucoin'.freeze # https://docs.coingecko.com/reference/exchanges-list
  ERRORS = {
    insufficient_funds: ['400100', 'Balance insufficient', 'Insufficient balance'],
    invalid_key: ['400004', 'Invalid KC-API-SIGN', 'Invalid KC-API-KEY', 'Invalid KC-API-PASSPHRASE']
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
    @proxy_ip ||= Clients::Kucoin::PROXY.split('://').last.split(':').first if Clients::Kucoin::PROXY.present?
  end

  def set_client(api_key: nil)
    @api_key = api_key
    @client = Clients::Kucoin.new(
      api_key: api_key&.key,
      api_secret: api_key&.secret,
      passphrase: api_key&.passphrase
    )
  end

  def get_tickers_info(force: false)
    cache_key = "exchange_#{id}_tickers_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour, force:) do
      result = client.get_symbols
      return Result::Failure.new("Failed to get #{name} symbols") if result.failure?

      return Result::Failure.new(result.data['msg']) if result.data['code'] != '200000'

      result.data['data'].map do |symbol_info|
        ticker = Utilities::Hash.dig_or_raise(symbol_info, 'symbol')
        base = Utilities::Hash.dig_or_raise(symbol_info, 'baseCurrency')
        quote = Utilities::Hash.dig_or_raise(symbol_info, 'quoteCurrency')
        enabled_trading = Utilities::Hash.dig_or_raise(symbol_info, 'enableTrading')

        base_increment = Utilities::Hash.dig_or_raise(symbol_info, 'baseIncrement')
        quote_increment = Utilities::Hash.dig_or_raise(symbol_info, 'quoteIncrement')
        price_increment = Utilities::Hash.dig_or_raise(symbol_info, 'priceIncrement')

        {
          ticker:,
          base:,
          quote:,
          minimum_base_size: Utilities::Hash.dig_or_raise(symbol_info, 'baseMinSize').to_d,
          minimum_quote_size: Utilities::Hash.dig_or_raise(symbol_info, 'quoteMinSize').to_d,
          maximum_base_size: Utilities::Hash.dig_or_raise(symbol_info, 'baseMaxSize').to_d,
          maximum_quote_size: Utilities::Hash.dig_or_raise(symbol_info, 'quoteMaxSize').to_d,
          base_decimals: Utilities::Number.decimals(base_increment),
          quote_decimals: Utilities::Number.decimals(quote_increment),
          price_decimals: Utilities::Number.decimals(price_increment),
          available: enabled_trading
        }
      end.compact
    end

    Result::Success.new(tickers_info)
  end

  def get_tickers_prices(force: false)
    cache_key = "exchange_#{id}_prices"
    tickers_prices = Rails.cache.fetch(cache_key, expires_in: 1.minute, force:) do
      result = client.get_all_tickers
      return Result::Failure.new("Failed to get #{name} tickers prices") if result.failure?

      return Result::Failure.new(result.data['msg']) if result.data['code'] != '200000'

      ticker_list = Utilities::Hash.dig_or_raise(result.data, 'data', 'ticker')
      ticker_list.each_with_object({}) do |ticker_data, prices_hash|
        symbol = Utilities::Hash.dig_or_raise(ticker_data, 'symbol')
        price = Utilities::Hash.dig_or_raise(ticker_data, 'last').to_d
        prices_hash[symbol] = price
      end
    end

    Result::Success.new(tickers_prices)
  end

  def get_balances(asset_ids: nil)
    result = client.get_accounts(type: 'trade')
    return result if result.failure?

    return Result::Failure.new(result.data['msg']) if result.data['code'] != '200000'

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.each_with_object({}) do |asset_id, balances_hash|
      balances_hash[asset_id] = { free: 0, locked: 0 }
    end

    accounts = Utilities::Hash.dig_or_raise(result.data, 'data')
    accounts.each do |account|
      currency = Utilities::Hash.dig_or_raise(account, 'currency')
      asset = asset_from_symbol(currency)
      next unless asset.present?
      next unless asset_ids.include?(asset.id)

      free = Utilities::Hash.dig_or_raise(account, 'available').to_d
      locked = Utilities::Hash.dig_or_raise(account, 'holds').to_d

      balances[asset.id] = { free:, locked: }
    end

    Result::Success.new(balances)
  end

  def get_last_price(ticker:, force: false)
    cache_key = "exchange_#{id}_last_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force:) do
      result = client.get_ticker(symbol: ticker.ticker)
      return result if result.failure?

      return Result::Failure.new(result.data['msg']) if result.data['code'] != '200000'

      price = Utilities::Hash.dig_or_raise(result.data, 'data', 'price').to_d
      raise "Wrong last price for #{ticker.ticker}: #{price}" if price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_bid_price(ticker:, force: false)
    cache_key = "exchange_#{id}_bid_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force:) do
      result = get_bid_ask_price(ticker)
      return result if result.failure?

      price = result.data[:bid][:price]
      raise "Wrong bid price for #{ticker.ticker}: #{price}" if price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_ask_price(ticker:, force: false)
    cache_key = "exchange_#{id}_ask_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force:) do
      result = get_bid_ask_price(ticker)
      return result if result.failure?

      price = result.data[:ask][:price]
      raise "Wrong ask price for #{ticker.ticker}: #{price}" if price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_candles(ticker:, start_at:, timeframe:)
    intervals = {
      1.minute => '1min',
      5.minutes => '5min',
      15.minutes => '15min',
      30.minutes => '30min',
      1.hour => '1hour',
      4.hours => '4hour',
      1.day => '1day',
      3.days => '1day',
      1.week => '1week',
      1.month => '1week'
      # 3.minutes => '3min',
      # 2.hours => '2hour',
      # 6.hours => '6hour',
      # 8.hours => '8hour',
      # 12.hours => '12hour',
    }
    interval = intervals[timeframe]

    duration = {
      '1min' => 1.minute,
      '3min' => 3.minutes,
      '5min' => 5.minutes,
      '15min' => 15.minutes,
      '30min' => 30.minutes,
      '1hour' => 1.hour,
      '2hour' => 2.hours,
      '4hour' => 4.hours,
      '6hour' => 6.hours,
      '8hour' => 8.hours,
      '12hour' => 12.hours,
      '1day' => 1.day,
      '1week' => 1.week
    }

    candles = []
    end_at = Time.now.utc

    loop do
      # Kucoin returns up to 1500 candles per request
      result = client.get_klines(
        symbol: ticker.ticker,
        type: interval,
        start_at: start_at.to_i,
        end_at: end_at.to_i
      )
      return result if result.failure?

      return Result::Failure.new(result.data['msg']) if result.data['code'] != '200000'

      raw_candles_list = result.data['data']
      break if raw_candles_list.empty?

      # Kucoin returns candles in reverse chronological order
      raw_candles_list.reverse.each do |candle|
        candles << [
          Time.at(candle[0].to_i).utc,
          candle[1].to_d,
          candle[3].to_d,
          candle[4].to_d,
          candle[2].to_d,
          candle[5].to_d
        ]
      end

      # If we got less than 1500 candles, we've reached the beginning
      break if raw_candles_list.length < 1500

      # Update end_at to the timestamp of the earliest candle we just fetched
      end_at = candles.first[0] - 1.second
      break if end_at <= start_at
    end

    candles = build_candles_from_candles(candles:, timeframe:) if timeframe.in?([3.days, 1.month])

    Result::Success.new(candles)
  end

  # @param amount_type [Symbol] :base or :quote
  def market_buy(ticker:, amount:, amount_type:)
    set_market_order(
      ticker:,
      amount:,
      amount_type:,
      side: :buy
    )
  end

  # @param amount_type [Symbol] :base or :quote
  def market_sell(ticker:, amount:, amount_type:)
    set_market_order(
      ticker:,
      amount:,
      amount_type:,
      side: :sell
    )
  end

  # @param amount_type [Symbol] :base or :quote
  def limit_buy(ticker:, amount:, amount_type:, price:)
    set_limit_order(
      ticker:,
      amount:,
      amount_type:,
      side: :buy,
      price:
    )
  end

  # @param amount_type [Symbol] :base or :quote
  def limit_sell(ticker:, amount:, amount_type:, price:)
    set_limit_order(
      ticker:,
      amount:,
      amount_type:,
      side: :sell,
      price:
    )
  end

  def get_order(order_id:)
    result = client.get_order(order_id:)
    return result if result.failure?

    return Result::Failure.new(result.data['msg']) if result.data['code'] != '200000'

    order_data = Utilities::Hash.dig_or_raise(result.data, 'data')
    normalized_order_data = parse_order_data(order_id, order_data)

    Result::Success.new(normalized_order_data)
  end

  def get_orders(order_ids:)
    orders = {}

    # Kucoin doesn't have a bulk get orders endpoint, so we need to fetch them one by one
    # We'll use the orders list endpoint with done status and filter by our order IDs
    order_ids.each do |order_id|
      result = get_order(order_id:)
      return result if result.failure?

      orders[order_id] = result.data
    end

    Result::Success.new(orders)
  end

  def cancel_order(order_id:)
    result = client.cancel_order(order_id:)
    return result if result.failure?

    return Result::Failure.new(result.data['msg']) if result.data['code'] != '200000'

    Result::Success.new(order_id)
  end

  def get_api_key_validity(api_key:)
    temp_client = Clients::Kucoin.new(
      api_key: api_key.key,
      api_secret: api_key.secret,
      passphrase: api_key.passphrase
    )

    # Try to get accounts to validate the API key
    result = temp_client.get_accounts(type: 'trade')

    if result.success? && result.data['code'] == '200000'
      # For Kucoin, we verify the key works by successfully fetching accounts
      # We don't have a specific permissions endpoint like other exchanges
      Result::Success.new(true)
    elsif result.data.present? && result.data['code'].in?(%w[400004 401001 401002 401003])
      # Invalid API key codes
      Result::Success.new(false)
    else
      result
    end
  end

  def minimum_amount_logic(**)
    :base_or_quote
  end

  private

  def client
    @client ||= set_client
  end

  def asset_from_symbol(symbol)
    @asset_from_symbol ||= tickers.available.includes(:base_asset, :quote_asset).each_with_object({}) do |t, h|
      h[t.base] ||= t.base_asset
      h[t.quote] ||= t.quote_asset
    end
    @asset_from_symbol[symbol]
  end

  def get_bid_ask_price(ticker)
    cache_key = "exchange_#{id}_bid_ask_price_#{ticker.id}"
    Rails.cache.fetch(cache_key, expires_in: 1.seconds) do
      result = client.get_order_book(symbol: ticker.ticker)
      return result if result.failure?

      return Result::Failure.new(result.data['msg']) if result.data['code'] != '200000'

      data = Utilities::Hash.dig_or_raise(result.data, 'data')

      Result::Success.new(
        {
          bid: {
            price: Utilities::Hash.dig_or_raise(data, 'bestBid').to_d,
            size: Utilities::Hash.dig_or_raise(data, 'bestBidSize').to_d
          },
          ask: {
            price: Utilities::Hash.dig_or_raise(data, 'bestAsk').to_d,
            size: Utilities::Hash.dig_or_raise(data, 'bestAskSize').to_d
          }
        }
      )
    end
  end

  # @param amount [Float] must be a positive number
  # @param amount_type [Symbol] :base or :quote
  # @param side [Symbol] must be either :buy or :sell
  def set_market_order(ticker:, amount:, amount_type:, side:)
    amount = ticker.adjusted_amount(amount:, amount_type:)

    order_settings = {
      client_oid: SecureRandom.uuid,
      symbol: ticker.ticker,
      side: side.to_s,
      type: 'market',
      size: amount_type == :base ? amount.to_d.to_s('F') : nil,
      funds: amount_type == :quote ? amount.to_d.to_s('F') : nil
    }.compact

    result = client.create_order(**order_settings)
    return result if result.failure?

    return Result::Failure.new(result.data['msg']) if result.data['code'] != '200000'

    data = {
      order_id: Utilities::Hash.dig_or_raise(result.data, 'data', 'orderId')
    }

    Result::Success.new(data)
  end

  # @param amount [Float] must be a positive number
  # @param amount_type [Symbol] :base or :quote
  # @param side [Symbol] must be either :buy or :sell
  # @param price [Float] must be a positive number
  def set_limit_order(ticker:, amount:, amount_type:, side:, price:)
    raise 'Kucoin does not support limit orders in quote currency' if amount_type == :quote

    amount = ticker.adjusted_amount(amount:, amount_type:)
    price = ticker.adjusted_price(price:)

    order_settings = {
      client_oid: SecureRandom.uuid,
      symbol: ticker.ticker,
      side: side.to_s,
      type: 'limit',
      price: price.to_d.to_s('F'),
      size: amount.to_d.to_s('F')
    }

    result = client.create_order(**order_settings)
    return result if result.failure?

    return Result::Failure.new(result.data['msg']) if result.data['code'] != '200000'

    data = {
      order_id: Utilities::Hash.dig_or_raise(result.data, 'data', 'orderId')
    }

    Result::Success.new(data)
  end

  def parse_order_data(order_id, order_data)
    symbol = Utilities::Hash.dig_or_raise(order_data, 'symbol')
    ticker = tickers.find_by(ticker: symbol)
    order_type = parse_order_type(Utilities::Hash.dig_or_raise(order_data, 'type'))
    price = Utilities::Hash.dig_or_raise(order_data, 'price').to_d
    if price.zero? && order_type == :market_order
      price = Utilities::Hash.dig_or_raise(order_data,
                                           'dealFunds').to_d / Utilities::Hash.dig_or_raise(order_data,
                                                                                            'dealSize').to_d
    end
    price = ticker.adjusted_price(price:, method: :round) if ticker.present? && price.positive?
    price = nil if price.zero?

    side = Utilities::Hash.dig_or_raise(order_data, 'side').downcase.to_sym
    amount = Utilities::Hash.dig_or_raise(order_data, 'size').to_d
    amount = amount.zero? ? nil : amount
    quote_amount = Utilities::Hash.dig_or_raise(order_data, 'funds').to_d
    quote_amount = quote_amount.zero? ? nil : quote_amount

    amount_exec = Utilities::Hash.dig_or_raise(order_data, 'dealSize').to_d
    quote_amount_exec = Utilities::Hash.dig_or_raise(order_data, 'dealFunds').to_d

    # Kucoin includes fees in dealSize and dealFunds
    fee = Utilities::Hash.dig_or_raise(order_data, 'fee').to_d
    fee_currency = Utilities::Hash.dig_or_raise(order_data, 'feeCurrency')

    # Adjust executed amounts based on fee currency
    if ticker.present?
      if fee_currency == ticker.base
        amount_exec = side == :buy ? (amount_exec - fee) : (amount_exec + fee)
      elsif fee_currency == ticker.quote
        quote_amount_exec = side == :buy ? (quote_amount_exec + fee) : (quote_amount_exec - fee)
      end
    end

    status = parse_order_status(Utilities::Hash.dig_or_raise(order_data, 'isActive'),
                                Utilities::Hash.dig_or_raise(order_data, 'cancelExist'))
    errors = [
      order_data['cancelledMsg'].presence
    ].compact

    {
      order_id:,
      ticker:,
      price:,
      amount:, # amount in the order config
      quote_amount:, # amount in the order config
      amount_exec:, # amount the account balance went up or down
      quote_amount_exec:, # amount the account balance went up or down
      side:,
      order_type:,
      error_messages: errors,
      status:,
      exchange_response: order_data
    }
  end

  def parse_order_type(order_type)
    case order_type
    when 'market'
      :market_order
    when 'limit'
      :limit_order
    else
      raise "Unknown #{name} order type: #{order_type}"
    end
  end

  def parse_order_status(is_active, _cancel_exist)
    # isActive: true if the order is active, false if done
    # cancelExist: true if the order was cancelled
    if is_active
      :open
    else
      :closed
    end
  end
end
