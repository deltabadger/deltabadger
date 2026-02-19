class Exchanges::Gemini < Exchange
  COINGECKO_ID = 'gemini'.freeze # https://docs.coingecko.com/reference/exchanges-list
  ERRORS = {
    insufficient_funds: ['InsufficientFunds', 'Insufficient Funds'],
    invalid_key: %w[InvalidSignature InvalidApiKey InvalidNonce]
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
    @client = Clients::Gemini.new(
      api_key: api_key&.key,
      api_secret: api_key&.secret
    )
  end

  def get_tickers_info(force: false)
    cache_key = "exchange_#{id}_tickers_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour, force: force) do
      result = client.get_symbols
      return result if result.failure?

      symbols = result.data
      return Result::Failure.new("Failed to get #{name} symbols") if symbols.nil?

      symbols.map do |symbol|
        detail_result = client.get_symbol_details(symbol: symbol)
        next if detail_result.failure?

        detail = detail_result.data
        base = Utilities::Hash.dig_or_raise(detail, 'base_currency').upcase
        quote = Utilities::Hash.dig_or_raise(detail, 'quote_currency').upcase
        status = Utilities::Hash.dig_or_raise(detail, 'status')
        tick_size = detail['tick_size']&.to_d || '0.01'.to_d
        quote_increment = detail['quote_increment']&.to_d || '0.01'.to_d
        min_order_size = Utilities::Hash.dig_or_raise(detail, 'min_order_size').to_d

        {
          ticker: symbol,
          base: base,
          quote: quote,
          minimum_base_size: min_order_size,
          minimum_quote_size: 0,
          maximum_base_size: nil,
          maximum_quote_size: nil,
          base_decimals: Utilities::Number.decimals(tick_size.to_s),
          quote_decimals: Utilities::Number.decimals(quote_increment.to_s),
          price_decimals: Utilities::Number.decimals(quote_increment.to_s),
          available: status == 'open'
        }
      end.compact
    end

    Result::Success.new(tickers_info)
  end

  def get_tickers_prices(force: false)
    cache_key = "exchange_#{id}_prices"
    tickers_prices = Rails.cache.fetch(cache_key, expires_in: 1.minute, force: force) do
      result = client.get_price_feed
      return result if result.failure?

      result.data.each_with_object({}) do |entry, prices_hash|
        pair = Utilities::Hash.dig_or_raise(entry, 'pair')
        price = Utilities::Hash.dig_or_raise(entry, 'price').to_d
        prices_hash[pair.downcase] = price
      end
    end

    Result::Success.new(tickers_prices)
  end

  def get_balances(asset_ids: nil)
    result = client.get_balances
    return result if result.failure?

    data = result.data
    return Result::Failure.new("Failed to get #{name} balances") if data.nil?

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.each_with_object({}) do |asset_id, balances_hash|
      balances_hash[asset_id] = { free: 0, locked: 0 }
    end
    data.each do |balance|
      currency = Utilities::Hash.dig_or_raise(balance, 'currency').upcase
      asset = asset_from_symbol(currency)
      next unless asset.present?
      next unless asset_ids.include?(asset.id)

      free = Utilities::Hash.dig_or_raise(balance, 'available').to_d
      locked = (Utilities::Hash.dig_or_raise(balance, 'amount').to_d - free)

      balances[asset.id] = { free: free, locked: locked }
    end

    Result::Success.new(balances)
  end

  def get_last_price(ticker:, force: false)
    cache_key = "exchange_#{id}_last_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      result = client.get_ticker(symbol: ticker.ticker)
      return result if result.failure?

      price = Utilities::Hash.dig_or_raise(result.data, 'last').to_d
      raise "Wrong last price for #{ticker.ticker}: #{price}" if price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_bid_price(ticker:, force: false)
    cache_key = "exchange_#{id}_bid_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
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
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      result = get_bid_ask_price(ticker)
      return result if result.failure?

      price = result.data[:ask][:price]
      raise "Wrong ask price for #{ticker.ticker}: #{price}" if price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_candles(ticker:, start_at:, timeframe:) # rubocop:disable Lint/UnusedMethodArgument
    # Gemini does not have a candle/OHLC endpoint in v1
    # Return empty candles as a fallback
    Result::Success.new([])
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
    result = client.order_status(order_id: order_id)
    return result if result.failure?

    order_data = result.data
    return Result::Failure.new("Failed to get #{name} order (order_id: #{order_id})") if order_data.nil?

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
    result = client.cancel_order(order_id: order_id)
    return result if result.failure?

    Result::Success.new(order_id)
  end

  def get_api_key_validity(api_key:)
    temp_client = Clients::Gemini.new(
      api_key: api_key.key,
      api_secret: api_key.secret
    )
    result = temp_client.get_balances

    if result.success? && result.data.is_a?(Array)
      Result::Success.new(true)
    elsif result.data.is_a?(Hash) && result.data[:status] == 400
      Result::Success.new(false)
    else
      result
    end
  end

  def minimum_amount_logic(**)
    :base
  end

  def withdraw(asset:, amount:, address:, network: nil, address_tag: nil) # rubocop:disable Lint/UnusedMethodArgument
    symbol = symbol_from_asset(asset)
    return Result::Failure.new("Unknown symbol for asset #{asset.symbol}") if symbol.blank?

    result = client.withdraw_crypto_funds(currency: symbol.downcase, address: address, amount: amount.to_d.to_s('F'))
    return result if result.failure?

    withdrawal_id = result.data['txHash'] || result.data['withdrawalId']
    Result::Success.new({ withdrawal_id: withdrawal_id })
  end

  def fetch_withdrawal_fees!
    Result::Success.new({})
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
      result = client.get_ticker(symbol: ticker.ticker)
      return result if result.failure?

      Result::Success.new(
        {
          bid: {
            price: Utilities::Hash.dig_or_raise(result.data, 'bid').to_d,
            size: 0
          },
          ask: {
            price: Utilities::Hash.dig_or_raise(result.data, 'ask').to_d,
            size: 0
          }
        }
      )
    end
  end

  # Gemini uses Immediate-or-Cancel for market-like orders
  # @param amount [Float] must be a positive number
  # @param amount_type [Symbol] :base or :quote
  # @param side [Symbol] must be either :buy or :sell
  def set_market_order(ticker:, amount:, amount_type:, side:)
    # Gemini doesn't support true market orders; use IOC limit order at aggressive price
    result = side == :buy ? get_ask_price(ticker: ticker) : get_bid_price(ticker: ticker)
    return result if result.failure?

    current_price = result.data
    # Set price 1% above ask for buy, 1% below bid for sell to ensure fill
    aggressive_price = side == :buy ? (current_price * 1.01) : (current_price * 0.99)
    aggressive_price = ticker.adjusted_price(price: aggressive_price)

    amount = ticker.adjusted_amount(amount: amount, amount_type: amount_type)
    # Convert quote amount to base using aggressive price
    if amount_type == :quote
      amount = (amount / aggressive_price)
      amount = ticker.adjusted_amount(amount: amount, amount_type: :base)
    end

    result = client.new_order(
      symbol: ticker.ticker,
      amount: amount.to_d.to_s('F'),
      price: aggressive_price.to_d.to_s('F'),
      side: side.to_s,
      type: 'exchange limit',
      options: ['immediate-or-cancel']
    )
    return result if result.failure?

    order_id = Utilities::Hash.dig_or_raise(result.data, 'order_id').to_s

    Result::Success.new({ order_id: order_id })
  end

  # @param amount [Float] must be a positive number
  # @param amount_type [Symbol] :base or :quote
  # @param side [Symbol] must be either :buy or :sell
  # @param price [Float] must be a positive number
  def set_limit_order(ticker:, amount:, amount_type:, side:, price:)
    amount = ticker.adjusted_amount(amount: amount, amount_type: amount_type)
    price = ticker.adjusted_price(price: price)

    result = client.new_order(
      symbol: ticker.ticker,
      amount: amount.to_d.to_s('F'),
      price: price.to_d.to_s('F'),
      side: side.to_s,
      type: 'exchange limit'
    )
    return result if result.failure?

    order_id = Utilities::Hash.dig_or_raise(result.data, 'order_id').to_s

    Result::Success.new({ order_id: order_id })
  end

  def parse_order_data(order_id, order_data)
    symbol = Utilities::Hash.dig_or_raise(order_data, 'symbol')
    ticker = tickers.find_by(ticker: symbol)
    order_type = parse_order_type(Utilities::Hash.dig_or_raise(order_data, 'type'))
    price = Utilities::Hash.dig_or_raise(order_data, 'avg_execution_price').to_d
    price = Utilities::Hash.dig_or_raise(order_data, 'price').to_d if price.zero?
    price = nil if price.zero?
    amount = Utilities::Hash.dig_or_raise(order_data, 'original_amount').to_d
    side = Utilities::Hash.dig_or_raise(order_data, 'side').downcase.to_sym
    amount_exec = Utilities::Hash.dig_or_raise(order_data, 'executed_amount').to_d
    quote_amount_exec = price.present? ? (amount_exec * price) : 0
    status = parse_order_status(order_data)

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
      error_messages: [],
      status: status,
      exchange_response: order_data
    }
  end

  def parse_order_type(order_type)
    case order_type.downcase
    when 'exchange limit', 'limit'
      :limit_order
    when 'market', 'exchange market'
      :market_order
    else
      # Gemini mostly uses limit orders, default to limit
      :limit_order
    end
  end

  def parse_order_status(order_data)
    is_live = order_data['is_live']
    is_cancelled = order_data['is_cancelled']
    executed_amount = order_data['executed_amount'].to_d
    remaining_amount = order_data['remaining_amount'].to_d

    if is_cancelled
      :cancelled
    elsif is_live && remaining_amount.positive?
      :open
    elsif !is_live && executed_amount.positive?
      :closed
    else
      :unknown
    end
  end
end
