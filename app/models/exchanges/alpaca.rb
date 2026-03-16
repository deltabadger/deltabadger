class Exchanges::Alpaca < Exchange
  ERRORS = {
    insufficient_funds: ['insufficient buying power']
  }.freeze

  include Exchange::Dryable

  attr_reader :api_key

  def coingecko_id
    nil
  end

  def known_errors
    ERRORS
  end

  def set_client(api_key: nil)
    @api_key = api_key
    @client = Clients::Alpaca.new(
      api_key: api_key&.key,
      api_secret: api_key&.secret,
      paper: paper_mode?(api_key)
    )
  end

  def requires_passphrase?
    true
  end

  def supports_withdrawal?
    false
  end

  def minimum_amount_logic(**)
    :quote
  end

  def market_open?
    clock = get_clock_cached
    return true if clock.nil?

    clock['is_open'] == true
  end

  def next_market_open_at
    clock = get_clock_cached
    return Time.current if clock.nil?

    Time.parse(clock['next_open'])
  end

  def get_tickers_info(force: false)
    cache_key = "exchange_#{id}_tickers_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour, force: force) do
      result = client.get_assets(status: 'active', asset_class: 'us_equity')
      return Result::Failure.new("Failed to get #{name} assets") if result.failure?

      result.data.select { |a| a['tradable'] && a['fractionable'] }.map do |asset|
        {
          ticker: asset['symbol'],
          base: asset['symbol'],
          quote: 'USD',
          minimum_base_size: 0.000000001.to_d, # fractional shares
          minimum_quote_size: 1.to_d, # $1 minimum
          maximum_base_size: 100_000.to_d,
          maximum_quote_size: 10_000_000.to_d,
          base_decimals: 9,
          quote_decimals: 2,
          price_decimals: 2,
          available: true
        }
      end
    end

    Result::Success.new(tickers_info)
  end

  def get_tickers_prices(force: false)
    cache_key = "exchange_#{id}_prices"
    tickers_prices = Rails.cache.fetch(cache_key, expires_in: 1.minute, force: force) do
      prices = {}
      tickers.available.pluck(:base).each do |symbol|
        result = client.get_latest_trade(symbol: symbol)
        next if result.failure?

        prices[symbol] = result.data.dig('trade', 'p').to_d
      end
      prices
    end

    Result::Success.new(tickers_prices)
  end

  def get_balances(asset_ids: nil)
    # Get cash balance from account
    account_result = client.get_account
    return account_result if account_result.failure?

    # Get stock positions
    positions_result = client.get_positions
    return positions_result if positions_result.failure?

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.to_h do |asset_id|
      [asset_id, { free: 0, locked: 0 }]
    end

    # USD cash
    usd_asset = asset_from_symbol('USD')
    if usd_asset && asset_ids.include?(usd_asset.id)
      cash = account_result.data['cash'].to_d
      balances[usd_asset.id] = { free: cash, locked: 0 }
    end

    # Stock positions
    positions_result.data.each do |position|
      asset = asset_from_symbol(position['symbol'])
      next unless asset && asset_ids.include?(asset.id)

      qty = position['qty'].to_d
      balances[asset.id] = { free: qty, locked: 0 }
    end

    Result::Success.new(balances)
  end

  def get_last_price(ticker:, force: false)
    cache_key = "exchange_#{id}_last_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      result = client.get_latest_trade(symbol: ticker.base)
      return result if result.failure?

      price = result.data.dig('trade', 'p').to_d
      raise "Wrong last price for #{ticker.base}: #{price}" if price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_bid_price(ticker:, force: false)
    cache_key = "exchange_#{id}_bid_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      result = client.get_latest_quote(symbol: ticker.base)
      return result if result.failure?

      price = result.data.dig('quote', 'bp').to_d
      raise "Wrong bid price for #{ticker.base}: #{price}" if price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_ask_price(ticker:, force: false)
    cache_key = "exchange_#{id}_ask_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      result = client.get_latest_quote(symbol: ticker.base)
      return result if result.failure?

      price = result.data.dig('quote', 'ap').to_d
      raise "Wrong ask price for #{ticker.base}: #{price}" if price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_candles(ticker:, start_at:, timeframe:)
    alpaca_timeframes = {
      1.minute => '1Min',
      5.minutes => '5Min',
      15.minutes => '15Min',
      30.minutes => '30Min',
      1.hour => '1Hour',
      4.hours => '4Hour',
      1.day => '1Day',
      1.week => '1Week',
      1.month => '1Month'
    }
    tf = alpaca_timeframes[timeframe] || '1Day'

    result = client.get_bars(
      symbol: ticker.base,
      timeframe: tf,
      start_time: start_at.iso8601
    )
    return result if result.failure?

    candles = (result.data['bars'] || []).map do |bar|
      [
        Time.parse(bar['t']).utc,
        bar['o'].to_d,
        bar['h'].to_d,
        bar['l'].to_d,
        bar['c'].to_d,
        bar['v'].to_d
      ]
    end

    Result::Success.new(candles)
  end

  def market_buy(ticker:, amount:, amount_type:)
    set_market_order(ticker: ticker, amount: amount, amount_type: amount_type, side: :buy)
  end

  def market_sell(ticker:, amount:, amount_type:)
    set_market_order(ticker: ticker, amount: amount, amount_type: amount_type, side: :sell)
  end

  def limit_buy(ticker:, amount:, amount_type:, price:)
    set_limit_order(ticker: ticker, amount: amount, amount_type: amount_type, side: :buy, price: price)
  end

  def limit_sell(ticker:, amount:, amount_type:, price:)
    set_limit_order(ticker: ticker, amount: amount, amount_type: amount_type, side: :sell, price: price)
  end

  def get_order(order_id:)
    result = client.get_order(order_id: order_id)
    return result if result.failure?

    normalized = parse_order_data(result.data)
    Result::Success.new(normalized)
  end

  def get_orders(order_ids:)
    orders = {}
    order_ids.each do |order_id|
      result = client.get_order(order_id: order_id)
      return result if result.failure?

      orders[order_id] = parse_order_data(result.data)
    end

    Result::Success.new(orders)
  end

  def list_open_orders
    result = client.list_orders(status: 'open')
    return result if result.failure?

    orders = result.data.map { |order| parse_order_data(order) }
    Result::Success.new(orders)
  end

  def cancel_order(order_id:)
    result = client.cancel_order(order_id: order_id)
    return result if result.failure?

    Result::Success.new(order_id)
  end

  def get_api_key_validity(api_key:)
    result = Clients::Alpaca.new(
      api_key: api_key.key,
      api_secret: api_key.secret,
      paper: paper_mode?(api_key)
    ).get_account

    if result.success?
      # Paper accounts return 'ACTIVE', live accounts also return 'ACTIVE'
      Result::Success.new(result.data['status'] == 'ACTIVE')
    elsif result.data.is_a?(Hash) && result.data[:status] == 401
      Result::Success.new(false)
    else
      result
    end
  end

  def fetch_withdrawal_fees!
    Result::Success.new({})
  end

  # Search tradable stocks from Alpaca API
  # @param query [String] search term
  # @return [Array<Hash>] matching stocks
  def search_assets(query)
    all_assets = get_cached_assets
    return [] if all_assets.blank? || query.blank?

    query_down = query.downcase
    all_assets.select do |a|
      a['symbol'].downcase.include?(query_down) ||
        a['name']&.downcase&.include?(query_down)
    end.first(20)
  end

  private

  def client
    @client ||= set_client
  end

  # Default to paper mode when passphrase is nil (safe default for testing)
  def paper_mode?(api_key)
    return true if api_key.nil?

    api_key.passphrase != 'live'
  end

  def get_clock_cached
    Rails.cache.fetch("exchange_#{id}_clock", expires_in: 1.minute) do
      result = client.get_clock
      return nil if result.failure?

      result.data
    end
  end

  def get_cached_assets
    Rails.cache.fetch("exchange_#{id}_tradable_assets", expires_in: 1.hour) do
      result = client.get_assets(status: 'active', asset_class: 'us_equity')
      return [] if result.failure?

      result.data.select { |a| a['tradable'] && a['fractionable'] }
    end
  end

  def asset_from_symbol(symbol)
    @asset_from_symbol ||= tickers.available.includes(:base_asset, :quote_asset).each_with_object({}) do |t, h|
      h[t.base] ||= t.base_asset
      h[t.quote] ||= t.quote_asset
    end
    @asset_from_symbol[symbol]
  end

  def set_market_order(ticker:, amount:, amount_type:, side:)
    result = if amount_type == :quote
               # Use notional (dollar amount) for market orders
               client.create_order(
                 symbol: ticker.base,
                 side: side.to_s,
                 type: 'market',
                 time_in_force: 'day',
                 notional: amount.to_d.to_s('F')
               )
             else
               client.create_order(
                 symbol: ticker.base,
                 side: side.to_s,
                 type: 'market',
                 time_in_force: 'day',
                 qty: amount.to_d.to_s('F')
               )
             end
    return result if result.failure?

    data = { order_id: result.data['id'] }
    Result::Success.new(data)
  end

  def set_limit_order(ticker:, amount:, amount_type:, side:, price:)
    # Limit orders require qty (shares), not notional
    qty = if amount_type == :quote
            (amount.to_d / price.to_d)
          else
            amount.to_d
          end

    price = ticker.adjusted_price(price: price)

    result = client.create_order(
      symbol: ticker.base,
      side: side.to_s,
      type: 'limit',
      time_in_force: 'gtc',
      qty: qty.to_d.to_s('F'),
      limit_price: price.to_d.to_s('F')
    )
    return result if result.failure?

    data = { order_id: result.data['id'] }
    Result::Success.new(data)
  end

  def parse_order_data(order_data)
    ticker_record = tickers.find_by(base: order_data['symbol'])
    order_type = order_data['type'] == 'limit' ? :limit_order : :market_order
    side = order_data['side']&.to_sym
    filled_qty = order_data['filled_qty'].to_d
    filled_avg_price = order_data['filled_avg_price']&.to_d
    notional = order_data['notional']&.to_d
    qty = order_data['qty']&.to_d
    limit_price = order_data['limit_price']&.to_d
    price = filled_avg_price.present? && filled_avg_price.positive? ? filled_avg_price : limit_price

    {
      order_id: order_data['id'],
      ticker: ticker_record,
      price: price,
      amount: qty,
      quote_amount: notional,
      amount_exec: filled_qty,
      quote_amount_exec: filled_qty * (filled_avg_price || 0),
      side: side,
      order_type: order_type,
      error_messages: [],
      status: parse_order_status(order_data['status']),
      exchange_response: order_data
    }
  end

  def parse_order_status(status)
    case status
    when 'new', 'accepted', 'pending_new'
      :open
    when 'filled'
      :closed
    when 'canceled', 'expired', 'replaced'
      :cancelled
    when 'rejected'
      :failed
    else
      :unknown
    end
  end
end
