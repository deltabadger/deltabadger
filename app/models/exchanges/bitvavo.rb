class Exchanges::Bitvavo < Exchange
  COINGECKO_ID = 'bitvavo'.freeze # https://docs.coingecko.com/reference/exchanges-list
  ERRORS = {
    insufficient_funds: ['Insufficient funds.'],
    invalid_key: ['Invalid API key.', 'Signature invalid.']
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
    @client = Clients::Bitvavo.new(
      api_key: api_key&.key,
      api_secret: api_key&.secret
    )
  end

  def get_tickers_info(force: false)
    cache_key = "exchange_#{id}_tickers_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour, force: force) do
      result = client.markets
      return result if result.failure?

      result.data.map do |product|
        market = Utilities::Hash.dig_or_raise(product, 'market')
        status = Utilities::Hash.dig_or_raise(product, 'status')
        base, quote = market.split('-')

        {
          ticker: market,
          base: base,
          quote: quote,
          minimum_base_size: product['minOrderInBaseAsset'].to_d,
          minimum_quote_size: product['minOrderInQuoteAsset'].to_d,
          maximum_base_size: nil,
          maximum_quote_size: nil,
          base_decimals: Utilities::Hash.dig_or_raise(product, 'orderTypes').include?('market') ? (product['pricePrecision'] || 8) : 8,
          quote_decimals: product['pricePrecision'] || 8,
          price_decimals: product['pricePrecision'] || 8,
          available: status == 'trading'
        }
      end.compact
    end

    Result::Success.new(tickers_info)
  end

  def get_tickers_prices(force: false)
    cache_key = "exchange_#{id}_prices"
    tickers_prices = Rails.cache.fetch(cache_key, expires_in: 1.minute, force: force) do
      result = client.ticker_price
      return result if result.failure?

      result.data.each_with_object({}) do |item, prices_hash|
        ticker = Utilities::Hash.dig_or_raise(item, 'market')
        price = item['price'].to_d
        prices_hash[ticker] = price if price.positive?
      end
    end

    Result::Success.new(tickers_prices)
  end

  def get_balances(asset_ids: nil)
    result = client.balance
    return result if result.failure?

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.each_with_object({}) do |asset_id, balances_hash|
      balances_hash[asset_id] = { free: 0, locked: 0 }
    end

    result.data.each do |balance_data|
      asset = asset_from_symbol(balance_data['symbol'])
      next unless asset.present?
      next unless asset_ids.include?(asset.id)

      free = Utilities::Hash.dig_or_raise(balance_data, 'available').to_d
      locked = Utilities::Hash.dig_or_raise(balance_data, 'inOrder').to_d

      balances[asset.id] = { free: free, locked: locked }
    end

    Result::Success.new(balances)
  end

  def get_last_price(ticker:, force: false)
    cache_key = "exchange_#{id}_last_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      result = client.ticker_price(market: ticker.ticker)
      return result if result.failure?

      price = Utilities::Hash.dig_or_raise(result.data, 'price').to_d
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

  def get_candles(ticker:, start_at:, timeframe:)
    intervals = {
      1.minute => '1m',
      5.minutes => '5m',
      15.minutes => '15m',
      30.minutes => '30m',
      1.hour => '1h',
      4.hours => '4h',
      1.day => '1d',
      3.days => '1d',
      1.week => '1d',
      1.month => '1d'
    }
    interval = intervals[timeframe]

    limit = 1440
    candles = []
    loop do
      result = client.candles(
        market: ticker.ticker,
        interval: interval,
        start_time: start_at.to_i * 1000,
        limit: limit
      )
      return result if result.failure?

      result.data.each do |candle|
        candles << [
          Time.at(candle[0].to_i / 1000).utc,
          candle[1].to_d,
          candle[2].to_d,
          candle[3].to_d,
          candle[4].to_d,
          candle[5].to_d
        ]
      end
      break if result.data.empty? || result.data.size < limit

      start_at = candles.last[0] + 1.second
    end

    candles = build_candles_from_candles(candles: candles, timeframe: timeframe) if timeframe.in?([3.days,
                                                                                                   1.week,
                                                                                                   1.month])

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
    # Bitvavo order_id format: "MARKET-orderId" e.g. "BTC-EUR-abc123"
    parts = order_id.split('-')
    market = parts[0..1].join('-')
    ext_order_id = parts[2..].join('-')
    result = client.get_order(market: market, order_id: ext_order_id)
    return result if result.failure?

    normalized_order_data = parse_order_data(order_id, result.data)

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
    parts = order_id.split('-')
    market = parts[0..1].join('-')
    ext_order_id = parts[2..].join('-')
    result = client.cancel_order(market: market, order_id: ext_order_id)
    return result if result.failure?

    Result::Success.new(order_id)
  end

  def get_api_key_validity(api_key:)
    result = Clients::Bitvavo.new(
      api_key: api_key.key,
      api_secret: api_key.secret
    ).balance

    if result.success?
      Result::Success.new(true)
    elsif result.data.is_a?(Hash) && result.data[:status] == 401
      Result::Success.new(false)
    else
      error_msg = result.errors.first
      if error_msg.present? && ERRORS[:invalid_key].any? { |msg| error_msg.include?(msg) }
        Result::Success.new(false)
      else
        result
      end
    end
  end

  def minimum_amount_logic(**)
    :base_or_quote
  end

  def withdraw(asset:, amount:, address:, network: nil, address_tag: nil) # rubocop:disable Lint/UnusedMethodArgument
    symbol = symbol_from_asset(asset)
    return Result::Failure.new("Unknown symbol for asset #{asset.symbol}") if symbol.blank?

    result = client.withdrawal(symbol: symbol, amount: amount.to_d.to_s('F'), address: address,
                               payment_id: address_tag)
    return result if result.failure?

    withdrawal_id = result.data['success'] ? "bitvavo-#{SecureRandom.uuid}" : nil
    Result::Success.new({ withdrawal_id: withdrawal_id })
  end

  def fetch_withdrawal_fees!
    result = Clients::Bitvavo.new.get_assets
    return result if result.failure?

    fees = {}
    chains = {}
    Array(result.data).each do |coin|
      symbol = coin['symbol']
      next if coin['withdrawalFee'].blank?

      fees[symbol] = coin['withdrawalFee']
      chains[symbol] = [{ 'name' => symbol, 'fee' => coin['withdrawalFee'], 'is_default' => true }]
    end

    update_exchange_asset_fees!(fees, chains: chains)
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
      result = client.ticker_book(market: ticker.ticker)
      return result if result.failure?

      formatted = {
        bid: {
          price: Utilities::Hash.dig_or_raise(result.data, 'bid').to_d,
          size: Utilities::Hash.dig_or_raise(result.data, 'bidSize').to_d
        },
        ask: {
          price: Utilities::Hash.dig_or_raise(result.data, 'ask').to_d,
          size: Utilities::Hash.dig_or_raise(result.data, 'askSize').to_d
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
      market: ticker.ticker,
      side: side.to_s,
      order_type: 'market',
      amount: amount_type == :base ? amount.to_d.to_s('F') : nil,
      amount_quote: amount_type == :quote ? amount.to_d.to_s('F') : nil
    }
    result = client.create_order(**order_settings)
    return result if result.failure?

    ext_order_id = Utilities::Hash.dig_or_raise(result.data, 'orderId')
    data = {
      order_id: "#{ticker.ticker}-#{ext_order_id}"
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
      market: ticker.ticker,
      side: side.to_s,
      order_type: 'limit',
      amount: amount.to_d.to_s('F'),
      price: price.to_d.to_s('F')
    }
    result = client.create_order(**order_settings)
    return result if result.failure?

    ext_order_id = Utilities::Hash.dig_or_raise(result.data, 'orderId')
    data = {
      order_id: "#{ticker.ticker}-#{ext_order_id}"
    }

    Result::Success.new(data)
  end

  def parse_order_data(order_id, order_data)
    market = Utilities::Hash.dig_or_raise(order_data, 'market')
    ticker = tickers.find_by(ticker: market)
    order_type = parse_order_type(Utilities::Hash.dig_or_raise(order_data, 'orderType'))
    price = order_data['price'].to_d
    side = Utilities::Hash.dig_or_raise(order_data, 'side').downcase.to_sym
    amount = order_data['amount'].to_d
    amount = nil if amount.zero?
    amount_quote = order_data['amountQuote'].to_d
    amount_quote = nil if amount_quote.zero?
    amount_exec = order_data['filledAmount'].to_d
    quote_amount_exec = order_data['filledAmountQuote'].to_d
    if price.zero? && quote_amount_exec.positive? && amount_exec.positive?
      price = quote_amount_exec / amount_exec
      price = ticker.adjusted_price(price: price, method: :round) if ticker.present?
    end
    price = nil if price.zero?
    status = parse_order_status(Utilities::Hash.dig_or_raise(order_data, 'status'))

    {
      order_id: order_id,
      ticker: ticker,
      price: price,
      amount: amount,
      quote_amount: amount_quote,
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
    when 'new', 'partiallyFilled'
      :open
    when 'filled'
      :closed
    when 'canceled', 'cancelled', 'expired'
      :cancelled
    when 'rejected'
      :failed
    else
      raise "Unknown #{name} order status: #{status}"
    end
  end
end
