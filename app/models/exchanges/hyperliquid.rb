class Exchanges::Hyperliquid < Exchange
  COINGECKO_ID = 'hyperliquid-spot'.freeze
  ERRORS = {
    insufficient_funds: ['Insufficient balance', 'Not enough balance'],
    invalid_key: ['Invalid API key', 'Authentication failed', 'Invalid signature']
  }.freeze

  # Hyperliquid price rule: at most SIGNIFICANT_FIGURES significant figures and,
  # for spot, at most (SPOT_MAX_DECIMALS - szDecimals) decimal places.
  SIGNIFICANT_FIGURES = 5
  SPOT_MAX_DECIMALS   = 8

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
    @client = Honeymaker.client('hyperliquid',
                                api_key: api_key&.key,
                                api_secret: api_key&.secret,
                                proxy: ENV['PROXY_HYPERLIQUID'])
  end

  def get_tickers_info(force: false)
    cache_key = "exchange_#{id}_tickers_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour, force:) do
      result = client.spot_meta
      return result if result.failure?

      tokens = result.data['tokens']
      universe = result.data['universe']
      token_map = tokens.to_h { |t| [t['index'], t] }

      universe.map do |pair|
        base_token = token_map[pair['tokens'][0]]
        quote_token = token_map[pair['tokens'][1]]
        next unless base_token && quote_token

        {
          ticker: pair['name'],
          base: base_token['name'],
          quote: quote_token['name'],
          minimum_base_size: 0,
          minimum_quote_size: 10, # Hyperliquid hard spot floor: orders below 10 USDC are rejected

          maximum_base_size: nil,
          maximum_quote_size: nil,
          base_decimals: base_token['szDecimals'] || 0,
          quote_decimals: 2, # USDC is always 2 decimals
          price_decimals: 5, # Hyperliquid uses up to 5 significant figures for prices
          available: true,
          trading_enabled: true
        }
      end.compact
    end

    Result::Success.new(tickers_info)
  end

  def get_tickers_prices(force: false, symbols: nil)
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
    balances = asset_ids.to_h do |asset_id|
      [asset_id, { free: 0, locked: 0 }]
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

  # Hyperliquid rejects prices that aren't on tick size: at most 5 significant
  # figures and, for spot, at most (8 - szDecimals) decimal places. We translate
  # the significant-figure rule into a decimal-place count so floor/ceil/round
  # semantics are preserved (a buy limit set below market must never round up).
  # BigDecimal#exponent gives the power-of-ten magnitude exactly: 66.6 => 2,
  # 3.5 => 1, 0.0234 => -1. szDecimals is stored on the ticker as base_decimals.
  def adjusted_price(ticker:, price:, method: :floor)
    price = price.to_d
    return price if price.zero?

    max_decimals = SPOT_MAX_DECIMALS - ticker.base_decimals.to_i
    sig_decimals = SIGNIFICANT_FIGURES - price.exponent
    decimals     = sig_decimals.clamp(0, max_decimals) # >=10k clamps to 0 dp; integers are always valid
    price.public_send(method, decimals).to_d           # to_d: floor/ceil/round(0) returns an Integer
  end

  def get_order(order_id:)
    _coin, oid = parse_order_id(order_id)
    result = client.order_status(user: api_key&.key, oid: oid.to_i)

    if result.failure?
      # honeymaker now centralizes parsing; a not_found is the distinct unknownOid signal. Aged-out
      # filled orders are normal on Hyperliquid → recover the fill BEFORE declaring not-found (mirrors
      # Kraken). A non-not_found failure (transient/throttle) is propagated so the job retries.
      return result unless result.data.is_a?(Hash) && result.data[:not_found]

      return recover_order_from_fills(order_id)
    end

    Result::Success.new(build_order_data(order_id, result.data))
  end

  def get_orders(order_ids:)
    orders = {}
    missing = []
    order_ids.each do |order_id|
      result = get_order(order_id: order_id)
      if result.failure?
        # not_found (incl. unknownOid with no recoverable fill) → collect under :missing for the
        # open-orders sweep; anything else (transient/throttle) → abort the batch so the job retries.
        if result.data.is_a?(Hash) && result.data[:not_found]
          missing << order_id
          next
        end

        return result
      end

      orders[order_id] = result.data
    end

    Result::Success.new(orders: orders, missing: missing)
  end

  # userFills exhausts Hyperliquid's fill history, so a still-missing order is confirmed
  # never-executed → Bot::FetchAndUpdateOpenOrdersJob may stop wedging on young missing ids
  # (limit DCA self-heals via missed_quote_amount). Same contract as Kraken.
  def authoritative_missing_orders?
    true
  end

  def cancel_order(order_id:)
    coin, oid = parse_order_id(order_id)
    result = client.cancel(coin: "#{coin}/USDC", oid: oid.to_i)
    return result if result.failure?

    Result::Success.new(order_id)
  end

  def get_api_key_validity(api_key:)
    result = Honeymaker.client('hyperliquid',
                               api_key: api_key.key,
                               api_secret: api_key.secret,
                               proxy: ENV['PROXY_HYPERLIQUID']).validate(:trading)
    Result::Success.new(result.success?)
  rescue StandardError
    # Network errors, etc. — don't fail the validation
    Result::Success.new(true)
  end

  def minimum_amount_logic(**)
    # Hyperliquid is limit-only spot with a hard 10-USDC order floor and no per-asset base floor
    # (minimum_base_size stays 0). :base_and_quote_in_base converts minimum_quote_size (10) into a
    # per-order base minimum, ceil(10 / limit_price), compared against the FLOORED base size — so an
    # order whose value rounds below 10 (incl. dual-asset sub-orders) is skipped up front instead of
    # being sent and hard-rejected by the exchange.
    :base_and_quote_in_base
  end

  def get_ledger(api_key:, start_time: nil)
    hm_client = Honeymaker.client('hyperliquid', api_key: api_key.key, api_secret: api_key.secret)
    start_ms = start_time ? (start_time.to_f * 1000).to_i : nil
    entries = []

    result = if start_ms
               hm_client.user_fills_by_time(user: api_key.key, start_time: start_ms)
             else
               hm_client.user_fills(user: api_key.key)
             end

    unless result.failure?
      Array(result.data).each do |fill|
        coin = fill['coin']
        is_buyer = fill['side'] == 'B'
        px = fill['px'].to_d
        sz = fill['sz'].to_d
        entries << { entry_type: is_buyer ? :buy : :sell,
                     base_currency: coin, base_amount: sz,
                     quote_currency: 'USDC', quote_amount: (px * sz),
                     fee_currency: 'USDC', fee_amount: fill['fee'].to_d.abs,
                     tx_id: fill['tid'].to_s, group_id: nil, description: nil,
                     transacted_at: Time.at(fill['time'].to_i / 1000.0).utc, raw_data: fill }
      end
    end

    Result::Success.new(entries)
  end

  private

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
    return Result::Failure.new('Hyperliquid order failed: limit price must be positive') unless price.positive?

    # Hyperliquid's order API takes size in BASE units, so a quote amount must be
    # converted at the limit price (mirrors Exchanges::Alpaca#set_limit_order).
    size = amount_type == :quote ? (amount.to_d / price.to_d) : amount.to_d
    size = ticker.adjusted_amount(amount: size, amount_type: :base)

    result = client.order(
      coin: ticker.ticker,
      is_buy: side == :buy,
      size: size.to_d,
      limit_px: price.to_d
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

  # honeymaker returns order_type as :limit/:market; the Transaction enum is :limit_order/:market_order.
  ORDER_TYPE_MAP = { limit: :limit_order, market: :market_order }.freeze

  # Build the Transaction-shaped order_data from honeymaker's already-normalized fields, adding only
  # what's local to this app: the resolved ticker (by the raw universe coin, e.g. "@142"), the enum
  # order_type, empty error_messages, and the raw exchange_response. The PASSED order_id is preserved
  # (honeymaker's id is "<raw coin>-<oid>"; ours is "<base>-<oid>").
  def build_order_data(order_id, data)
    {
      order_id:,
      ticker: tickers.find_by(ticker: data[:coin]),
      price: data[:price],
      amount: data[:amount],
      quote_amount: data[:quote_amount],
      amount_exec: data[:amount_exec],
      quote_amount_exec: data[:quote_amount_exec],
      side: data[:side],
      order_type: ORDER_TYPE_MAP.fetch(data[:order_type], :limit_order),
      error_messages: [],
      status: data[:status],
      exchange_response: data[:raw]
    }
  end

  # unknownOid is normal for aged filled/canceled orders. Recover from userFillsByTime keyed on THIS
  # order's oid, bounding the lookback by the order's own row (mirrors Kraken's recover_missing_from_trades:
  # userFillsByTime needs a start_time, and get_order has none). Returns a synthesized :closed when
  # execution is proven, else a distinct not_found signal (so FetchAndUpdateOrderJob's StaleOrderResolver
  # path fires). A transient userFills failure is propagated, NOT degraded to not_found.
  def recover_order_from_fills(order_id)
    _coin, oid = parse_order_id(order_id)
    since = transactions.where(external_id: order_id).minimum(:created_at)
    return not_found_failure(order_id) unless since

    start_ms = ((since - 1.hour).to_f * 1000).to_i # ms, buffered for immediate-fill / clock skew
    result = client.user_fills_by_time(user: api_key&.key, start_time: start_ms)
    return result if result.failure?

    fills = Array(result.data).select { |f| f['oid'].to_s == oid.to_s }
    return not_found_failure(order_id) if fills.empty?

    Result::Success.new(order_data_from_fills(order_id, fills))
  end

  def not_found_failure(order_id)
    Result::Failure.new("Hyperliquid did not return data for order #{order_id}",
                        data: { not_found: true, missing_ids: [order_id] })
  end

  def order_data_from_fills(order_id, fills)
    amount_exec = fills.sum(BigDecimal('0')) { |f| f['sz'].to_d }
    quote_amount_exec = fills.sum(BigDecimal('0')) { |f| f['px'].to_d * f['sz'].to_d }
    {
      order_id:,
      ticker: tickers.find_by(ticker: fills.first['coin']),
      price: amount_exec.positive? ? (quote_amount_exec / amount_exec) : nil,
      amount: nil,
      quote_amount: nil,
      amount_exec:,
      quote_amount_exec:,
      side: fills.first['side'] == 'B' ? :buy : :sell,
      order_type: :limit_order,
      error_messages: [],
      status: :closed,
      exchange_response: { 'fills' => fills }
    }
  end

  # Suffix-aware so the whole Hyperliquid cancel family maps correctly; a triggered order has fired
  # and become a live resting order → :open. Production parsing now lives in honeymaker; this mirror is
  # retained for the parse_order_status characterization suite. An unmapped status is logged, not raised.
  def parse_order_status(status)
    case status
    when 'filled'
      :closed
    when 'open', 'triggered'
      :open
    else
      str = status.to_s
      if str.match?(/cancel/i) || str.match?(/reject/i)
        :cancelled
      else
        Rails.logger.warn("[Hyperliquid] Unmapped order status: #{status.inspect}")
        :unknown
      end
    end
  end
end
