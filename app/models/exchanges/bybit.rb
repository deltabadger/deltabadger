class Exchanges::Bybit < Exchange
  COINGECKO_ID = 'bybit_spot'.freeze # https://docs.coingecko.com/reference/exchanges-list
  ERRORS = {
    insufficient_funds: ['170131', 'Insufficient balance'],
    invalid_key: %w[10003 10004]
  }.freeze

  include Exchange::Dryable # decorators for: get_order, get_orders, cancel_order, get_api_key_validity, set_market_order, set_limit_order

  attr_reader :api_key

  def coingecko_id
    COINGECKO_ID
  end

  def known_errors
    ERRORS
  end

  def set_client(api_key: nil)
    @api_key = api_key
    @client = Clients::Bybit.new(
      api_key: api_key&.key,
      api_secret: api_key&.secret
    )
  end

  def get_tickers_info(force: false)
    cache_key = "exchange_#{id}_tickers_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour, force: force) do
      result = client.instruments_info(category: 'spot')
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error) : result
      end

      ret_code = result.data['retCode']
      return Result::Failure.new(result.data['retMsg']) if ret_code != 0

      items = Utilities::Hash.dig_or_raise(result.data, 'result', 'list')
      items.map do |product|
        ticker = Utilities::Hash.dig_or_raise(product, 'symbol')
        status = Utilities::Hash.dig_or_raise(product, 'status')

        lot_size_filter = Utilities::Hash.dig_or_raise(product, 'lotSizeFilter')
        price_filter = Utilities::Hash.dig_or_raise(product, 'priceFilter')

        {
          ticker: ticker,
          base: Utilities::Hash.dig_or_raise(product, 'baseCoin'),
          quote: Utilities::Hash.dig_or_raise(product, 'quoteCoin'),
          minimum_base_size: lot_size_filter['minOrderQty'].to_d,
          minimum_quote_size: lot_size_filter['minOrderAmt'].to_d,
          maximum_base_size: lot_size_filter['maxOrderQty'].to_d,
          maximum_quote_size: lot_size_filter['maxOrderAmt'].to_d,
          base_decimals: Utilities::Number.decimals(lot_size_filter['basePrecision']),
          quote_decimals: Utilities::Number.decimals(lot_size_filter['quotePrecision']),
          price_decimals: Utilities::Number.decimals(price_filter['tickSize']),
          available: status == 'Trading'
        }
      end.compact
    end

    Result::Success.new(tickers_info)
  end

  def get_tickers_prices(force: false)
    cache_key = "exchange_#{id}_prices"
    tickers_prices = Rails.cache.fetch(cache_key, expires_in: 1.minute, force: force) do
      result = client.tickers(category: 'spot')
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error) : result
      end

      ret_code = result.data['retCode']
      return Result::Failure.new(result.data['retMsg']) if ret_code != 0

      items = Utilities::Hash.dig_or_raise(result.data, 'result', 'list')
      items.each_with_object({}) do |item, prices_hash|
        ticker = Utilities::Hash.dig_or_raise(item, 'symbol')
        price = Utilities::Hash.dig_or_raise(item, 'lastPrice').to_d
        prices_hash[ticker] = price
      end
    end

    Result::Success.new(tickers_prices)
  end

  def get_balances(asset_ids: nil)
    result = client.wallet_balance(account_type: 'UNIFIED')
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

    ret_code = result.data['retCode']
    return Result::Failure.new(result.data['retMsg']) if ret_code != 0

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.each_with_object({}) do |asset_id, balances_hash|
      balances_hash[asset_id] = { free: 0, locked: 0 }
    end

    accounts = Utilities::Hash.dig_or_raise(result.data, 'result', 'list')
    accounts.each do |account|
      coins = Utilities::Hash.dig_or_raise(account, 'coin')
      coins.each do |coin|
        asset = asset_from_symbol(coin['coin'])
        next unless asset.present?
        next unless asset_ids.include?(asset.id)

        free = coin['availableToWithdraw'].to_d
        locked = coin['locked'].to_d
        balances[asset.id] = { free: free, locked: locked }
      end
    end

    Result::Success.new(balances)
  end

  def get_last_price(ticker:, force: false)
    cache_key = "exchange_#{id}_last_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      result = client.tickers(category: 'spot', symbol: ticker.ticker)
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error) : result
      end

      ret_code = result.data['retCode']
      return Result::Failure.new(result.data['retMsg']) if ret_code != 0

      items = Utilities::Hash.dig_or_raise(result.data, 'result', 'list')
      price = Utilities::Hash.dig_or_raise(items.first, 'lastPrice').to_d
      raise "Wrong last price for #{ticker.ticker}: #{price}" if price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_bid_price(ticker:, force: false)
    cache_key = "exchange_#{id}_bid_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      result = get_bid_ask_price(ticker)
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error) : result
      end

      price = result.data[:bid][:price]
      raise "Wrong bid price for #{ticker.ticker}: #{price}" if price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_ask_price(ticker:, force: false)
    cache_key = "exchange_#{id}_ask_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
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
      1.minute => '1',
      5.minutes => '5',
      15.minutes => '15',
      30.minutes => '30',
      1.hour => '60',
      4.hours => '240',
      1.day => 'D',
      3.days => 'D',
      1.week => 'W',
      1.month => 'M'
    }
    interval = intervals[timeframe]

    limit = 1000
    candles = []
    loop do
      result = client.kline(
        category: 'spot',
        symbol: ticker.ticker,
        interval: interval,
        start: start_at.to_i * 1000,
        limit: limit
      )
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error) : result
      end

      ret_code = result.data['retCode']
      return Result::Failure.new(result.data['retMsg']) if ret_code != 0

      items = Utilities::Hash.dig_or_raise(result.data, 'result', 'list')
      # Bybit returns candles in reverse chronological order
      items.reverse_each do |candle|
        candles << [
          Time.at(candle[0].to_i / 1000).utc,
          candle[1].to_d,
          candle[2].to_d,
          candle[3].to_d,
          candle[4].to_d,
          candle[5].to_d
        ]
      end
      break if items.empty? || items.size < limit

      start_at = candles.last[0] + 1.second
    end

    candles = build_candles_from_candles(candles: candles, timeframe: timeframe) if timeframe.in?([3.days])

    Result::Success.new(candles)
  end

  # @param amount_type [Symbol] :base or :quote
  def market_buy(ticker:, amount:, amount_type:)
    set_market_order(
      ticker: ticker,
      amount: amount,
      amount_type: amount_type,
      side: :buy
    )
  end

  # @param amount_type [Symbol] :base or :quote
  def market_sell(ticker:, amount:, amount_type:)
    set_market_order(
      ticker: ticker,
      amount: amount,
      amount_type: amount_type,
      side: :sell
    )
  end

  # @param amount_type [Symbol] :base or :quote
  def limit_buy(ticker:, amount:, amount_type:, price:)
    set_limit_order(
      ticker: ticker,
      amount: amount,
      amount_type: amount_type,
      side: :buy,
      price: price
    )
  end

  # @param amount_type [Symbol] :base or :quote
  def limit_sell(ticker:, amount:, amount_type:, price:)
    set_limit_order(
      ticker: ticker,
      amount: amount,
      amount_type: amount_type,
      side: :sell,
      price: price
    )
  end

  def get_order(order_id:)
    result = client.get_order(category: 'spot', order_id: order_id)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

    ret_code = result.data['retCode']
    return Result::Failure.new(result.data['retMsg']) if ret_code != 0

    order_list = Utilities::Hash.dig_or_raise(result.data, 'result', 'list')
    return Result::Failure.new("Failed to get #{name} order (order_id: #{order_id})") if order_list.empty?

    normalized_order_data = parse_order_data(order_id, order_list.first)

    Result::Success.new(normalized_order_data)
  end

  def get_orders(order_ids:)
    orders = {}
    order_ids.each do |order_id|
      result = get_order(order_id: order_id)
      return result if result.failure?

      orders[order_id] = result.data
    end

    Result::Success.new(orders)
  end

  def cancel_order(order_id:)
    # Need to find the symbol for the order
    get_result = client.get_order(category: 'spot', order_id: order_id)
    if get_result.failure?
      error = parse_error_message(get_result)
      return error.present? ? Result::Failure.new(error) : get_result
    end

    order_list = Utilities::Hash.dig_or_raise(get_result.data, 'result', 'list')
    return Result::Failure.new("Failed to find #{name} order for cancellation") if order_list.empty?

    symbol = Utilities::Hash.dig_or_raise(order_list.first, 'symbol')
    result = client.cancel_order(category: 'spot', symbol: symbol, order_id: order_id)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

    ret_code = result.data['retCode']
    return Result::Failure.new(result.data['retMsg']) if ret_code != 0

    Result::Success.new(order_id)
  end

  def get_api_key_validity(api_key:)
    result = Clients::Bybit.new(
      api_key: api_key.key,
      api_secret: api_key.secret
    ).wallet_balance(account_type: 'UNIFIED')

    if result.success?
      ret_code = result.data['retCode']
      if ret_code.zero?
        Result::Success.new(true)
      elsif ret_code.to_s.in?(ERRORS[:invalid_key])
        Result::Success.new(false)
      else
        Result::Failure.new(result.data['retMsg'])
      end
    else
      error = parse_error_message(result)
      if error.present? && ERRORS[:invalid_key].any? { |msg| error.include?(msg) }
        Result::Success.new(false)
      else
        result
      end
    end
  end

  def minimum_amount_logic(**)
    :base_or_quote
  end

  def withdraw(asset:, amount:, address:, network: nil, address_tag: nil)
    symbol = symbol_from_asset(asset)
    return Result::Failure.new("Unknown symbol for asset #{asset.symbol}") if symbol.blank?

    # Use provided network or determine the default chain
    chain_name = network
    if chain_name.blank?
      coin_result = client.get_coin_query_info
      return coin_result if coin_result.failure?

      rows = coin_result.data.dig('result', 'rows') || []
      coin_data = rows.find { |r| r['coin'] == symbol }
      return Result::Failure.new("No coin data found for #{symbol} on Bybit") if coin_data.blank?

      chains = coin_data['chains'] || []
      chain = chains.find { |c| c['chainDefault'] == '1' } || chains.first
      return Result::Failure.new("No chain found for #{symbol} on Bybit") if chain.blank?

      chain_name = chain['chain']
    end

    result = client.withdraw(coin: symbol, chain: chain_name, address: address,
                             amount: amount.to_d.to_s('F'), tag: address_tag)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

    withdrawal_id = result.data.dig('result', 'id')
    Result::Success.new({ withdrawal_id: withdrawal_id })
  end

  def fetch_withdrawal_fees!
    api_key = fee_api_key
    return Result::Success.new({}) if api_key.blank?

    result = Clients::Bybit.new(
      api_key: api_key.key,
      api_secret: api_key.secret
    ).get_coin_query_info
    return result if result.failure?

    fees = {}
    chain_data = {}
    rows = result.data.dig('result', 'rows') || []
    rows.each do |coin|
      symbol = coin['coin']
      coin_chains = coin['chains'] || []
      chain = coin_chains.find { |c| c['chainDefault'] == '1' } || coin_chains.first
      next unless chain

      fees[symbol] = chain['withdrawFee']
      chain_data[symbol] = coin_chains.map do |c|
        { 'name' => c['chain'], 'fee' => c['withdrawFee'], 'is_default' => c['chainDefault'] == '1' }
      end
    end

    update_exchange_asset_fees!(fees, chains: chain_data)
  end

  private

  def client
    @client ||= set_client
  end

  def parse_error_message(result)
    return unless result.errors.first.present?

    begin
      parsed = JSON.parse(result.errors.first)
      parsed['retMsg'] || parsed['msg']
    rescue StandardError
      nil
    end
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
      result = client.orderbook(category: 'spot', symbol: ticker.ticker, limit: 1)
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error) : result
      end

      ret_code = result.data['retCode']
      return Result::Failure.new(result.data['retMsg']) if ret_code != 0

      book = Utilities::Hash.dig_or_raise(result.data, 'result')
      bids = Utilities::Hash.dig_or_raise(book, 'b')
      asks = Utilities::Hash.dig_or_raise(book, 'a')

      formatted = {
        bid: {
          price: bids.first[0].to_d,
          size: bids.first[1].to_d
        },
        ask: {
          price: asks.first[0].to_d,
          size: asks.first[1].to_d
        }
      }
      Result::Success.new(formatted)
    end
  end

  # @param amount [Float] must be a positive number
  # @param amount_type [Symbol] :base or :quote
  # @param side [Symbol] must be either :buy or :sell
  def set_market_order(ticker:, amount:, amount_type:, side:)
    amount = ticker.adjusted_amount(amount: amount, amount_type: amount_type)

    order_settings = {
      category: 'spot',
      symbol: ticker.ticker,
      side: side.to_s.capitalize,
      order_type: 'Market',
      qty: amount.to_d.to_s('F'),
      market_unit: amount_type == :quote ? 'quoteCoin' : 'baseCoin'
    }
    result = client.create_order(**order_settings)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

    ret_code = result.data['retCode']
    return Result::Failure.new(result.data['retMsg']) if ret_code != 0

    data = {
      order_id: Utilities::Hash.dig_or_raise(result.data, 'result', 'orderId')
    }

    Result::Success.new(data)
  end

  # @param amount [Float] must be a positive number
  # @param amount_type [Symbol] :base or :quote
  # @param side [Symbol] must be either :buy or :sell
  # @param price [Float] must be a positive number
  def set_limit_order(ticker:, amount:, amount_type:, side:, price:)
    amount = ticker.adjusted_amount(amount: amount, amount_type: amount_type)
    price = ticker.adjusted_price(price: price)

    order_settings = {
      category: 'spot',
      symbol: ticker.ticker,
      side: side.to_s.capitalize,
      order_type: 'Limit',
      qty: amount.to_d.to_s('F'),
      price: price.to_d.to_s('F'),
      time_in_force: 'GTC'
    }
    result = client.create_order(**order_settings)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

    ret_code = result.data['retCode']
    return Result::Failure.new(result.data['retMsg']) if ret_code != 0

    data = {
      order_id: Utilities::Hash.dig_or_raise(result.data, 'result', 'orderId')
    }

    Result::Success.new(data)
  end

  def parse_order_data(order_id, order_data)
    symbol = Utilities::Hash.dig_or_raise(order_data, 'symbol')
    ticker = tickers.find_by(ticker: symbol)
    order_type = parse_order_type(Utilities::Hash.dig_or_raise(order_data, 'orderType'))
    price = Utilities::Hash.dig_or_raise(order_data, 'avgPrice').to_d
    price = Utilities::Hash.dig_or_raise(order_data, 'price').to_d if price.zero?
    price = nil if price.zero?
    side = Utilities::Hash.dig_or_raise(order_data, 'side').downcase.to_sym
    amount = Utilities::Hash.dig_or_raise(order_data, 'qty').to_d
    amount = nil if amount.zero?
    amount_exec = Utilities::Hash.dig_or_raise(order_data, 'cumExecQty').to_d
    quote_amount_exec = Utilities::Hash.dig_or_raise(order_data, 'cumExecValue').to_d
    status = parse_order_status(Utilities::Hash.dig_or_raise(order_data, 'orderStatus'))

    {
      order_id: order_id,
      ticker: ticker,
      price: price,
      amount: amount,
      quote_amount: nil,
      amount_exec: amount_exec,
      quote_amount_exec: quote_amount_exec,
      side: side,
      order_type: order_type,
      error_messages: [order_data['rejectReason'].presence].compact.reject { |r| r == 'EC_NoError' },
      status: status,
      exchange_response: order_data
    }
  end

  def parse_order_type(order_type)
    case order_type
    when 'Market'
      :market_order
    when 'Limit'
      :limit_order
    else
      raise "Unknown #{name} order type: #{order_type}"
    end
  end

  def parse_order_status(status)
    # https://bybit-exchange.github.io/docs/v5/enum#orderstatus
    case status
    when 'Created', 'Untriggered'
      :unknown
    when 'New', 'PartiallyFilled', 'PartiallyFilledCanceled'
      :open
    when 'Filled'
      :closed
    when 'Cancelled', 'Expired', 'Deactivated'
      :cancelled
    when 'Rejected'
      :failed
    else
      raise "Unknown #{name} order status: #{status}"
    end
  end
end
