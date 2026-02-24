class Exchanges::Hyperliquid < Exchange
  COINGECKO_ID = 'hyperliquid-spot'.freeze
  ERRORS = {
    insufficient_funds: ['Insufficient balance', 'Not enough balance'],
    invalid_key: ['Invalid API key', 'Authentication failed', 'Invalid signature']
  }.freeze

  include Exchange::Dryable

  attr_reader :api_key

  def coingecko_id
    COINGECKO_ID
  end

  def known_errors
    ERRORS
  end

  def supports_withdrawal?
    false
  end

  def set_client(api_key: nil)
    @api_key = api_key
    @client = Clients::Hyperliquid.new(
      wallet_address: api_key&.key,
      agent_key: api_key&.secret
    )
  end

  def get_tickers_info(force: false)
    cache_key = "exchange_#{id}_tickers_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour, force:) do
      result = client.spot_meta
      return result if result.failure?

      tokens = result.data['tokens']
      universe = result.data['universe']
      token_map = tokens.each_with_object({}) { |t, h| h[t['index']] = t }

      universe.map do |pair|
        base_token = token_map[pair['tokens'][0]]
        quote_token = token_map[pair['tokens'][1]]
        next unless base_token && quote_token

        {
          ticker: pair['name'],
          base: base_token['name'],
          quote: quote_token['name'],
          minimum_base_size: 0,
          minimum_quote_size: 0,
          maximum_base_size: nil,
          maximum_quote_size: nil,
          base_decimals: base_token['szDecimals'] || 0,
          quote_decimals: 2, # USDC is always 2 decimals
          price_decimals: 5, # Hyperliquid uses up to 5 significant figures for prices
          available: true
        }
      end.compact
    end

    Result::Success.new(tickers_info)
  end

  def get_tickers_prices(force: false)
    cache_key = "exchange_#{id}_prices"
    tickers_prices = Rails.cache.fetch(cache_key, expires_in: 1.minute, force:) do
      result = client.all_mids
      return result if result.failure?

      # Filter to only tickers we have in our DB
      available_tickers = tickers.available.pluck(:ticker)
      result.data.select { |ticker, _| ticker.in?(available_tickers) }
                 .transform_values(&:to_d)
    end

    Result::Success.new(tickers_prices)
  end

  def get_balances(asset_ids: nil)
    result = client.spot_balances
    return result if result.failure?

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.each_with_object({}) do |asset_id, balances_hash|
      balances_hash[asset_id] = { free: 0, locked: 0 }
    end

    balance_list = result.data['balances'] || []
    balance_list.each do |balance|
      asset = asset_from_symbol(balance['coin'])
      next unless asset.present?
      next unless asset_ids.include?(asset.id)

      total = balance['total'].to_d
      hold = (balance['hold'] || '0').to_d
      free = total - hold
      balances[asset.id] = { free:, locked: hold }
    end

    Result::Success.new(balances)
  end

  def get_last_price(ticker:, force: false)
    cache_key = "exchange_#{id}_last_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force:) do
      result = client.all_mids
      return result if result.failure?

      price = result.data[ticker.ticker]&.to_d
      raise "No price available for #{ticker.ticker}" if price.nil? || price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_bid_price(ticker:, force: false)
    cache_key = "exchange_#{id}_bid_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force:) do
      result = get_l2_book(ticker)
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
      result = get_l2_book(ticker)
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

    start_time_ms = start_at.to_i * 1000
    end_time_ms = Time.now.utc.to_i * 1000

    result = client.candles_snapshot(
      coin: ticker.ticker,
      interval: interval,
      start_time: start_time_ms,
      end_time: end_time_ms
    )
    return result if result.failure?

    candles = result.data.map do |candle|
      [
        Time.at(candle['t'].to_i / 1000).utc,
        candle['o'].to_d,
        candle['h'].to_d,
        candle['l'].to_d,
        candle['c'].to_d,
        candle['v'].to_d
      ]
    end

    candles = build_candles_from_candles(candles:, timeframe:) if timeframe.in?([3.days, 1.week, 1.month])

    Result::Success.new(candles)
  end

  # Hyperliquid spot has no native market orders — limit orders are always used.
  # The LimitOrderable concern handles this by always using limit orders.
  def market_buy(**)
    raise 'Hyperliquid does not support market orders on spot. Use limit orders instead.'
  end

  def market_sell(**)
    raise 'Hyperliquid does not support market orders on spot. Use limit orders instead.'
  end

  def limit_buy(ticker:, amount:, amount_type:, price:)
    set_limit_order(
      ticker:,
      amount:,
      amount_type:,
      side: :buy,
      price:
    )
  end

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
    _coin, oid = parse_order_id(order_id)
    result = client.order_status(oid: oid.to_i)
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
    coin, oid = parse_order_id(order_id)
    result = client.cancel(coin: "#{coin}/USDC", oid: oid.to_i)
    return result if result.failure?

    Result::Success.new(order_id)
  end

  def get_api_key_validity(api_key:)
    temp_client = Clients::Hyperliquid.new(
      wallet_address: api_key.key,
      agent_key: api_key.secret
    )
    # Try cancelling a non-existent order. Valid key = order not found error.
    # Invalid key = authentication error.
    temp_client.cancel(coin: 'ETH', oid: 0)
    # If we get here (success or non-auth failure), the key is valid
    Result::Success.new(true)
  rescue Hyperliquid::AuthenticationError
    Result::Success.new(false)
  rescue StandardError
    # Network errors, etc. — don't fail the validation
    Result::Success.new(true)
  end

  def minimum_amount_logic(**)
    :base
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

  def get_l2_book(ticker)
    cache_key = "exchange_#{id}_l2_book_#{ticker.id}"
    Rails.cache.fetch(cache_key, expires_in: 1.seconds) do
      result = client.l2_book(coin: ticker.ticker)
      return result if result.failure?

      levels = result.data['levels']
      bids = levels[0] # bids array
      asks = levels[1] # asks array

      formatted = {
        bid: {
          price: bids.first['px'].to_d,
          size: bids.first['sz'].to_d
        },
        ask: {
          price: asks.first['px'].to_d,
          size: asks.first['sz'].to_d
        }
      }
      Result::Success.new(formatted)
    end
  end

  # Order ID format: "COIN-oid" (e.g., "PURR-123456")
  def build_order_id(coin, oid)
    "#{coin}-#{oid}"
  end

  def parse_order_id(order_id)
    parts = order_id.split('-', 2)
    [parts[0], parts[1]]
  end

  def set_limit_order(ticker:, amount:, amount_type:, side:, price:)
    amount = ticker.adjusted_amount(amount:, amount_type:)
    price = ticker.adjusted_price(price:)

    result = client.order(
      coin: ticker.ticker,
      is_buy: side == :buy,
      size: amount.to_d.to_s('F'),
      limit_px: price.to_d.to_s('F')
    )
    return result if result.failure?

    response = result.data
    return Result::Failure.new("Hyperliquid order failed: #{response}") unless response['status'] == 'ok'

    statuses = response.dig('response', 'data', 'statuses')
    return Result::Failure.new('Hyperliquid order failed: no statuses returned') if statuses.blank?

    order_status = statuses.first
    return Result::Failure.new("Hyperliquid order failed: #{order_status['error']}") if order_status['error'].present?

    # Order could be resting (limit) or filled immediately
    oid = order_status.dig('resting', 'oid') || order_status.dig('filled', 'oid')
    return Result::Failure.new('Hyperliquid order failed: no order ID returned') if oid.nil?

    base_coin = ticker.base
    data = { order_id: build_order_id(base_coin, oid) }

    Result::Success.new(data)
  end

  def parse_order_data(order_id, data)
    order = data['order'] || {}
    status_str = data['status']
    fills = data['fills'] || []

    coin = order['coin']
    ticker_record = tickers.find_by(ticker: coin)

    side = order['side'] == 'B' ? :buy : :sell
    order_type = :limit_order # Hyperliquid spot only has limit orders
    limit_price = order['limitPx']&.to_d
    ordered_size = order['sz']&.to_d

    # Calculate executed amounts from fills
    amount_exec = fills.sum { |f| f['sz'].to_d }
    quote_amount_exec = fills.sum { |f| f['px'].to_d * f['sz'].to_d }
    avg_price = amount_exec.positive? ? (quote_amount_exec / amount_exec) : limit_price

    {
      order_id:,
      ticker: ticker_record,
      price: avg_price,
      amount: ordered_size,
      quote_amount: nil,
      amount_exec:,
      quote_amount_exec:,
      side:,
      order_type:,
      error_messages: [],
      status: parse_order_status(status_str),
      exchange_response: data
    }
  end

  def parse_order_status(status)
    case status
    when 'open', 'marginCanceled'
      :open
    when 'filled'
      :closed
    when 'canceled', 'triggered', 'rejected'
      :cancelled
    when 'unknownOid'
      :unknown
    else
      :unknown
    end
  end
end
