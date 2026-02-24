class Exchanges::BitMart < Exchange
  COINGECKO_ID = 'bitmart'.freeze # https://docs.coingecko.com/reference/exchanges-list
  ERRORS = {
    insufficient_funds: ['Balance not enough', 'Insufficient balance'],
    invalid_key: ['Invalid ACCESS_KEY', 'Invalid sign', 'Header X-BM-KEY Is Empty']
  }.freeze

  include Exchange::Dryable # decorators for: get_order, get_orders, cancel_order, get_api_key_validity, set_market_order, set_limit_order

  attr_reader :api_key

  def coingecko_id
    COINGECKO_ID
  end

  def known_errors
    ERRORS
  end

  def requires_passphrase?
    true
  end

  def set_client(api_key: nil)
    @api_key = api_key
    @client = Clients::BitMart.new(
      api_key: api_key&.key,
      api_secret: api_key&.secret,
      memo: api_key&.passphrase
    )
  end

  def get_tickers_info(force: false)
    cache_key = "exchange_#{id}_tickers_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour, force: force) do
      result = client.get_symbols
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error) : result
      end

      return Result::Failure.new(result.data['message']) if result.data['code'] != 1000

      items = Utilities::Hash.dig_or_raise(result.data, 'data', 'symbols')
      items.filter_map do |product|
        ticker = Utilities::Hash.dig_or_raise(product, 'symbol')
        trade_status = Utilities::Hash.dig_or_raise(product, 'trade_status')

        {
          ticker: ticker,
          base: Utilities::Hash.dig_or_raise(product, 'base_currency'),
          quote: Utilities::Hash.dig_or_raise(product, 'quote_currency'),
          minimum_base_size: product['base_min_size'].to_d,
          minimum_quote_size: product['min_buy_amount'].to_d,
          maximum_base_size: 0.to_d,
          maximum_quote_size: 0.to_d,
          base_decimals: Utilities::Number.decimals(product['base_min_size']),
          quote_decimals: Utilities::Number.decimals(product['quote_increment']),
          price_decimals: product['price_max_precision'].to_i,
          available: trade_status == 'trading'
        }
      end
    end

    Result::Success.new(tickers_info)
  end

  def get_tickers_prices(force: false)
    cache_key = "exchange_#{id}_prices"
    tickers_prices = Rails.cache.fetch(cache_key, expires_in: 1.minute, force: force) do
      result = client.get_ticker
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error) : result
      end

      return Result::Failure.new(result.data['message']) if result.data['code'] != 1000

      items = Utilities::Hash.dig_or_raise(result.data, 'data')
      items.each_with_object({}) do |item, prices_hash|
        ticker = Utilities::Hash.dig_or_raise(item, 'symbol')
        price = Utilities::Hash.dig_or_raise(item, 'last').to_d
        prices_hash[ticker] = price
      end
    end

    Result::Success.new(tickers_prices)
  end

  def get_balances(asset_ids: nil)
    result = client.get_wallet
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

    return Result::Failure.new(result.data['message']) if result.data['code'] != 1000

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.each_with_object({}) do |asset_id, balances_hash|
      balances_hash[asset_id] = { free: 0, locked: 0 }
    end

    raw_balances = Utilities::Hash.dig_or_raise(result.data, 'data', 'wallet')
    raw_balances.each do |balance|
      asset = asset_from_symbol(balance['id'])
      next unless asset.present?
      next unless asset_ids.include?(asset.id)

      free = Utilities::Hash.dig_or_raise(balance, 'available').to_d
      locked = Utilities::Hash.dig_or_raise(balance, 'frozen').to_d
      balances[asset.id] = { free: free, locked: locked }
    end

    Result::Success.new(balances)
  end

  def get_last_price(ticker:, force: false)
    cache_key = "exchange_#{id}_last_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      result = client.get_ticker(symbol: ticker.ticker)
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error) : result
      end

      return Result::Failure.new(result.data['message']) if result.data['code'] != 1000

      item = Utilities::Hash.dig_or_raise(result.data, 'data')
      price = Utilities::Hash.dig_or_raise(item, 'last').to_d
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
    # BitMart kline steps are in minutes
    intervals = {
      1.minute => 1,
      5.minutes => 5,
      15.minutes => 15,
      30.minutes => 30,
      1.hour => 60,
      4.hours => 240,
      1.day => 1440,
      3.days => 4320,
      1.week => 10_080,
      1.month => 43_200
    }
    step = intervals[timeframe]

    limit = 500
    candles = []
    loop do
      result = client.get_klines(
        symbol: ticker.ticker,
        step: step,
        after: start_at.to_i,
        limit: limit
      )
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error) : result
      end

      items = result.data.is_a?(Hash) ? (result.data['data'] || []) : []
      items.each do |candle|
        candles << [
          Time.at(candle[0].to_i).utc,
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
    result = client.get_order(order_id: order_id)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

    return Result::Failure.new(result.data['message']) if result.data['code'] != 1000

    order_data = Utilities::Hash.dig_or_raise(result.data, 'data')
    normalized_order_data = parse_order_data(order_id, order_data)

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
    get_result = client.get_order(order_id: order_id)
    if get_result.failure?
      error = parse_error_message(get_result)
      return error.present? ? Result::Failure.new(error) : get_result
    end

    symbol = Utilities::Hash.dig_or_raise(get_result.data, 'data', 'symbol')
    result = client.cancel_order(symbol: symbol, order_id: order_id)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

    Result::Success.new(order_id)
  end

  def get_api_key_validity(api_key:)
    result = Clients::BitMart.new(
      api_key: api_key.key,
      api_secret: api_key.secret,
      memo: api_key.passphrase
    ).get_wallet

    if result.success?
      if result.data['code'] == 1000
        Result::Success.new(true)
      else
        error_msg = result.data['message']
        if ERRORS[:invalid_key].any? { |msg| error_msg&.include?(msg) }
          Result::Success.new(false)
        else
          Result::Failure.new(error_msg)
        end
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
    :base_and_quote
  end

  def withdraw(asset:, amount:, address:, network: nil, address_tag: nil)
    symbol = symbol_from_asset(asset)
    return Result::Failure.new("Unknown symbol for asset #{asset.symbol}") if symbol.blank?

    result = client.withdraw(currency: symbol, amount: amount.to_d.to_s('F'),
                             address: address, network: network, address_memo: address_tag)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

    return Result::Failure.new(result.data['message']) if result.data['code'] != 1000

    withdrawal_id = result.data.dig('data', 'withdraw_id')
    Result::Success.new({ withdrawal_id: withdrawal_id })
  end

  def fetch_withdrawal_fees!
    result = Clients::BitMart.new.get_currencies
    return result if result.failure?

    return Result::Failure.new(result.data['message']) if result.data['code'] != 1000

    fees = {}
    chains = {}
    currencies = result.data.dig('data', 'currencies') || []
    currencies.each do |currency|
      symbol = currency['currency']
      networks = currency['network'] || []
      default_net = networks.first
      next unless default_net

      fees[symbol] = default_net['withdraw_minfee']
      chains[symbol] = networks.map.with_index do |n, i|
        { 'name' => n['network'], 'fee' => n['withdraw_minfee'], 'is_default' => i.zero? }
      end
    end

    update_exchange_asset_fees!(fees, chains: chains)
  end

  private

  def client
    @client ||= set_client
  end

  def parse_error_message(result)
    return unless result.errors.first.present?

    begin
      JSON.parse(result.errors.first)['message']
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
      result = client.get_depth(symbol: ticker.ticker, limit: 1)
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error) : result
      end

      return Result::Failure.new(result.data['message']) if result.data['code'] != 1000

      book = Utilities::Hash.dig_or_raise(result.data, 'data')
      bids = Utilities::Hash.dig_or_raise(book, 'bids')
      asks = Utilities::Hash.dig_or_raise(book, 'asks')

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
      symbol: ticker.ticker,
      side: side.to_s,
      type: 'market',
      notional: amount_type == :quote ? amount.to_d.to_s('F') : nil,
      size: amount_type == :base ? amount.to_d.to_s('F') : nil
    }
    result = client.create_order(**order_settings)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

    return Result::Failure.new(result.data['message']) if result.data['code'] != 1000

    ext_order_id = Utilities::Hash.dig_or_raise(result.data, 'data', 'order_id')
    data = {
      order_id: ext_order_id.to_s
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
      symbol: ticker.ticker,
      side: side.to_s,
      type: 'limit',
      price: price.to_d.to_s('F'),
      size: amount_type == :base ? amount.to_d.to_s('F') : nil
    }
    result = client.create_order(**order_settings)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

    return Result::Failure.new(result.data['message']) if result.data['code'] != 1000

    ext_order_id = Utilities::Hash.dig_or_raise(result.data, 'data', 'order_id')
    data = {
      order_id: ext_order_id.to_s
    }

    Result::Success.new(data)
  end

  def parse_order_data(order_id, order_data)
    symbol = Utilities::Hash.dig_or_raise(order_data, 'symbol')
    ticker = tickers.find_by(ticker: symbol)
    order_type = parse_order_type(Utilities::Hash.dig_or_raise(order_data, 'type'))
    side = Utilities::Hash.dig_or_raise(order_data, 'side').downcase.to_sym
    amount = order_data['size'].to_d
    amount = nil if amount.zero?
    quote_amount = order_data['notional']&.to_d
    quote_amount = nil if quote_amount.nil? || quote_amount.zero?
    amount_exec = order_data['filled_size'].to_d
    quote_amount_exec = order_data['filled_notional'].to_d
    quote_amount_exec = nil if quote_amount_exec.negative?
    status = parse_order_status(Utilities::Hash.dig_or_raise(order_data, 'status'))
    price = order_data['price'].to_d
    if price.zero? &&
       quote_amount_exec.present? &&
       quote_amount_exec.positive? &&
       amount_exec.positive?
      price = quote_amount_exec / amount_exec
      price = ticker.adjusted_price(price: price, method: :round) if ticker.present?
    end
    price = nil if price.zero?

    {
      order_id: order_id,
      ticker: ticker,
      price: price,
      amount: amount,
      quote_amount: quote_amount,
      amount_exec: amount_exec,
      quote_amount_exec: quote_amount_exec,
      side: side,
      order_type: order_type,
      error_messages: [],
      status: status,
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

  def parse_order_status(status)
    case status
    when 'new', 'partially_filled'
      :open
    when 'filled'
      :closed
    when 'canceled', 'expired', 'partially_canceled'
      :cancelled
    when 'rejected', 'failed'
      :failed
    else
      raise "Unknown #{name} order status: #{status}"
    end
  end
end
