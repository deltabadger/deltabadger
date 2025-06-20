module Exchange::Exchanges::Kraken
  extend ActiveSupport::Concern

  COINGECKO_ID = 'kraken'.freeze # https://docs.coingecko.com/reference/exchanges-list
  TICKER_BLACKLIST = [].freeze
  ASSET_MAP = {
    'ZUSD' => 'USD',
    'ZEUR' => 'EUR',
    'ZGBP' => 'GBP',
    'ZJPY' => 'JPY',
    'ZCHF' => 'CHF',
    'ZCAD' => 'CAD',
    'ZAUD' => 'AUD',
    'XXBT' => 'XBT',
    'XETH' => 'ETH',
    'XXDG' => 'XDG'
  }.freeze # matches how assets are shown in the balances response with how they are shown in the tickers
  REAL_COSTMIN = {
    'AUD' => 10,     # instead of 1
    'CAD' => 5,      # instead of 1
    'CHF' => 5,      # instead of 1
    'DAI' => 5,      # instead of 0.5
    'ETH' => 0.002,  # instead of 0.0002
    'EUR' => 0.5,    # instead of 0.45
    'GBP' => 5,      # instead of 0.43
    'JPY' => 500,    # instead of 50
    'PYUSD' => 5,    # instead of 0.5
    'RLUSD' => 5,    # instead of 0.5
    'USD' => 5,      # instead of 0.5
    'USDC' => 5,     # instead of 0.5
    'USDQ' => 5,     # instead of 0.5
    'USDR' => 5,     # instead of 0.5
    'USDT' => 5,     # instead of 0.5
    'XBT' => 0.00005 # instead of 0.00002
  }.freeze # the real costmin values that Kraken respects. Found by trial and error using test orders.
  ERRORS = {
    insufficient_funds: ['EAPI:Insufficient funds', 'EOrder:Insufficient funds'],
    invalid_key: ['EGeneral:Permission denied', 'EAPI:Invalid key', 'EAPI:Invalid signature']
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
    @proxy_ip ||= KrakenClient::PROXY.split('://').last.split(':').first if KrakenClient::PROXY.present?
  end

  def set_client(api_key: nil)
    @api_key = api_key
    @client = KrakenClient.new(
      api_key: api_key&.key,
      api_secret: api_key&.secret
    )
  end

  def get_tickers_info(force: false)
    cache_key = "exchange_#{id}_tickers_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour, force: force) do
      result = client.get_tradable_asset_pairs
      return result if result.failure?

      error = Utilities::Hash.dig_or_raise(result.data, 'error')
      return Result::Failure.new(*error) if error.any?

      result.data['result'].map do |_, info|
        ticker = Utilities::Hash.dig_or_raise(info, 'altname')
        next if TICKER_BLACKLIST.include?(ticker)

        wsname = Utilities::Hash.dig_or_raise(info, 'wsname')
        base, quote = wsname.split('/')
        minimum_base_size = Utilities::Hash.dig_or_raise(info, 'ordermin').to_d
        minimum_quote_size = (REAL_COSTMIN[quote] || Utilities::Hash.dig_or_raise(info, 'costmin')).to_d
        {
          ticker: ticker,
          base: base,
          quote: quote,
          minimum_base_size: minimum_base_size,
          minimum_quote_size: minimum_quote_size,
          maximum_base_size: nil,
          maximum_quote_size: nil,
          base_decimals: Utilities::Hash.dig_or_raise(info, 'lot_decimals'),
          quote_decimals: Utilities::Hash.dig_or_raise(info, 'cost_decimals'),
          price_decimals: Utilities::Hash.dig_or_raise(info, 'pair_decimals')
        }
      end.compact
    end

    Result::Success.new(tickers_info)
  end

  def get_tickers_prices(force: false)
    cache_key = "exchange_#{id}_prices"
    tickers_prices = Rails.cache.fetch(cache_key, expires_in: 1.minute, force: force) do
      result = client.get_ticker_information
      return result if result.failure?

      error = Utilities::Hash.dig_or_raise(result.data, 'error')
      return Result::Failure.new(*error) if error.any?

      prices_hash = {}
      result.data['result'].each do |data|
        ticker = data[0]
        price = Utilities::Hash.dig_or_raise(data[1], 'c')[0].to_d
        prices_hash[ticker] = price
      end

      missing_tickers = tickers.pluck(:ticker) - prices_hash.keys
      missing_tickers.each do |ticker|
        result = client.get_ticker_information(pair: ticker)
        return result if result.failure?

        error = Utilities::Hash.dig_or_raise(result.data, 'error')
        return Result::Failure.new(*error) if error.any?

        asset_ticker_info = Utilities::Hash.dig_or_raise(result.data, 'result').map { |_, v| v }.first
        return Result::Failure.new("Failed to get #{name} tickers prices (ticker: #{ticker})") if asset_ticker_info.nil?

        price = Utilities::Hash.dig_or_raise(asset_ticker_info, 'c')[0].to_d
        prices_hash[ticker] = price
      end

      prices_hash
    end

    Result::Success.new(tickers_prices)
  end

  def get_balances(asset_ids: nil)
    result = client.get_extended_balance
    return result if result.failure?

    error = Utilities::Hash.dig_or_raise(result.data, 'error')
    return Result::Failure.new(*error) if error.any?

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.each_with_object({}) do |asset_id, balances_hash|
      balances_hash[asset_id] = { free: 0, locked: 0 }
    end
    balances_data = Utilities::Hash.dig_or_raise(result.data, 'result')
    balances_data.each do |asset, balance|
      asset = asset_from_symbol(symbol: asset)
      next unless asset.present?
      next unless asset_ids.include?(asset.id)

      total = Utilities::Hash.dig_or_raise(balance, 'balance').to_d
      locked = Utilities::Hash.dig_or_raise(balance, 'hold_trade').to_d
      free = total - locked
      balances[asset.id] = { free: free, locked: locked }
    end

    Result::Success.new(balances)
  end

  def get_last_price(ticker:, force: false)
    cache_key = "exchange_#{id}_last_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      result = get_ticker_information(ticker: ticker)
      return result if result.failure?

      price = result.data[:last_trade_closed][:price]
      raise "Wrong last price for #{ticker.ticker}: #{price}" if price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_bid_price(ticker:, force: false)
    cache_key = "exchange_#{id}_bid_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      result = get_ticker_information(ticker: ticker)
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
      result = get_ticker_information(ticker: ticker)
      return result if result.failure?

      price = result.data[:ask][:price]
      raise "Wrong ask price for #{ticker.ticker}: #{price}" if price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_candles(ticker:, start_at:, timeframe:)
    # Kraken only returns up to 720 of the most recent entries
    # (older data cannot be retrieved, regardless of the value of start_at)
    intervals = {
      1.minute => 1,
      5.minutes => 5,
      15.minutes => 15,
      30.minutes => 30,
      1.hour => 60,
      4.hours => 240,
      1.day => 1440,
      3.days => 1440,
      1.week => 1440,
      1.month => 1440
      # 1.week => 10_080,  # doesn't follow TradingView start timestamp
      # 15.days => 21_600  # doesn't follow TradingView start timestamp
    }
    interval = intervals[timeframe]

    candles = []
    start_at -= 1.second
    result = client.get_ohlc_data(
      pair: ticker.ticker,
      interval: interval,
      since: start_at.to_i
    )
    return result if result.failure?

    error = Utilities::Hash.dig_or_raise(result.data, 'error')
    return Result::Failure.new(*error) if error.any?

    raw_candles_list = result.data['result'][(result.data['result'].keys - ['last'])[0]]
    raw_candles_list.each do |candle|
      new_candle = [
        Time.at(candle[0]).utc, # start
        candle[1].to_d, # open
        candle[2].to_d, # high
        candle[3].to_d, # low
        candle[4].to_d, # close
        # candle[5].to_d, # vwap
        candle[6].to_d # volume
        # candle[7].to_d  # count
      ]
      candles << new_candle if new_candle[0] >= start_at
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
    result = client.query_orders_info(txid: order_id)
    return result if result.failure?

    error = Utilities::Hash.dig_or_raise(result.data, 'error')
    return Result::Failure.new(*error) if error.any?

    order_data = Utilities::Hash.dig_or_raise(result.data, 'result')[order_id]
    return Result::Failure.new("Failed to get #{name} order (order_id: #{order_id}). Order data is nil") if order_data.nil?

    normalized_order_data = parse_order_data(order_id, order_data)

    Result::Success.new(normalized_order_data)
  end

  def get_orders(order_ids:)
    orders = {}
    order_ids.each_slice(50) do |order_ids_slice|
      result = client.query_orders_info(txid: order_ids_slice.join(','))
      return result if result.failure?

      error = Utilities::Hash.dig_or_raise(result.data, 'error')
      return Result::Failure.new(*error) if error.any?

      order_datas = Utilities::Hash.dig_or_raise(result.data, 'result')
      order_datas.each do |order_id, order_data|
        orders[order_id] = parse_order_data(order_id, order_data)
      end
    end

    Result::Success.new(orders)
  end

  def cancel_order(order_id:)
    result = client.cancel_order(txid: order_id)
    return result if result.failure?

    error = Utilities::Hash.dig_or_raise(result.data, 'error')
    return Result::Failure.new(*error) if error.any?

    Result::Success.new(order_id)
  end

  def get_api_key_validity(api_key:)
    temp_client = KrakenClient.new(
      api_key: api_key.key,
      api_secret: api_key.secret
    )
    result = if api_key.trading?
               temp_client.add_order(
                 ordertype: 'market',
                 type: 'buy',
                 volume: 100,
                 pair: 'XBTUSD',
                 oflags: ['viqc'],
                 validate: true
               )
             elsif api_key.withdrawal?
               temp_client.get_withdrawal_methods
             else
               raise StandardError, 'Invalid API key type'
             end
    return result if result.failure?

    errors = Utilities::Hash.dig_or_raise(result.data, 'error')
    if errors.empty?
      Result::Success.new(true)
    elsif errors.first.in?(ERRORS[:invalid_key])
      Result::Success.new(false)
    else
      Result::Failure.new(*errors)
    end
  end

  def minimum_amount_logic(side:, order_type:)
    if side == :buy && order_type == :market_order
      :base_or_quote
    else
      :base
    end
  end

  private

  def client
    @client ||= set_client
  end

  def asset_from_symbol(symbol:)
    @asset_from_symbol ||= tickers.includes(:base_asset, :quote_asset).each_with_object({}) do |t, map|
      map[t.base] ||= t.base_asset
      map[t.quote] ||= t.quote_asset
    end
    symbol = symbol.split('.').first
    symbol = ASSET_MAP[symbol] || symbol
    @asset_from_symbol[symbol]
  end

  def get_ticker_information(ticker:) # rubocop:disable Metrics/MethodLength
    cache_key = "exchange_#{id}_ticker_information_#{ticker.ticker}"
    Rails.cache.fetch(cache_key, expires_in: 1.seconds) do # rubocop:disable Metrics/BlockLength
      result = client.get_ticker_information(pair: ticker.ticker)
      return result if result.failure?

      error = Utilities::Hash.dig_or_raise(result.data, 'error')
      return Result::Failure.new(*error) if error.any?

      asset_ticker_info = Utilities::Hash.dig_or_raise(result.data, 'result').map { |_, v| v }.first
      return Result::Failure.new("Failed to get #{name} #{ticker.ticker} ticker information") if asset_ticker_info.nil?

      formatted_asset_ticker_info = {
        ask: {
          price: Utilities::Hash.dig_or_raise(asset_ticker_info, 'a')[0].to_d,
          whole_lot_volume: Utilities::Hash.dig_or_raise(asset_ticker_info, 'a')[1].to_d,
          lot_volume: Utilities::Hash.dig_or_raise(asset_ticker_info, 'a')[2].to_d
        },
        bid: {
          price: Utilities::Hash.dig_or_raise(asset_ticker_info, 'b')[0].to_d,
          whole_lot_volume: Utilities::Hash.dig_or_raise(asset_ticker_info, 'b')[1].to_d,
          lot_volume: Utilities::Hash.dig_or_raise(asset_ticker_info, 'b')[2].to_d
        },
        last_trade_closed: {
          price: Utilities::Hash.dig_or_raise(asset_ticker_info, 'c')[0].to_d,
          lot_volume: Utilities::Hash.dig_or_raise(asset_ticker_info, 'c')[1].to_d
        },
        volume: {
          today: Utilities::Hash.dig_or_raise(asset_ticker_info, 'v')[0].to_d,
          last_24_hours: Utilities::Hash.dig_or_raise(asset_ticker_info, 'v')[1].to_d
        },
        volume_weighted_average_price: {
          today: Utilities::Hash.dig_or_raise(asset_ticker_info, 'p')[0].to_d,
          last_24_hours: Utilities::Hash.dig_or_raise(asset_ticker_info, 'p')[1].to_d
        },
        number_of_trades: {
          today: Utilities::Hash.dig_or_raise(asset_ticker_info, 't')[0].to_i,
          last_24_hours: Utilities::Hash.dig_or_raise(asset_ticker_info, 't')[1].to_i
        },
        low: {
          today: Utilities::Hash.dig_or_raise(asset_ticker_info, 'l')[0].to_d,
          last_24_hours: Utilities::Hash.dig_or_raise(asset_ticker_info, 'l')[1].to_d
        },
        high: {
          today: Utilities::Hash.dig_or_raise(asset_ticker_info, 'h')[0].to_d,
          last_24_hours: Utilities::Hash.dig_or_raise(asset_ticker_info, 'h')[1].to_d
        },
        todays_opening_price: Utilities::Hash.dig_or_raise(asset_ticker_info, 'o').to_d
      }
      Result::Success.new(formatted_asset_ticker_info)
    end
  end

  # @param amount [Float] must be a positive number
  # @param amount_type [Symbol] :base or :quote
  # @param side [Symbol] must be either :buy or :sell
  def set_market_order(ticker:, amount:, amount_type:, side:)
    raise 'Kraken does not support market sell orders in quote currency' if side == :sell && amount_type == :quote

    amount = ticker.adjusted_amount(amount: amount, amount_type: amount_type)

    client_order_id = SecureRandom.uuid
    order_settings = {
      cl_ord_id: client_order_id,
      ordertype: 'market',
      type: side.to_s.downcase,
      volume: amount.to_d.to_s('F'),
      pair: ticker.ticker,
      oflags: amount_type == :quote ? ['viqc'] : []
    }
    Rails.logger.info("Exchange #{id}: Setting market order #{order_settings.inspect}")
    result = client.add_order(**order_settings)
    return result if result.failure?

    error = Utilities::Hash.dig_or_raise(result.data, 'error')
    return Result::Failure.new(*error) if error.any?

    order_id = Utilities::Hash.dig_or_raise(result.data, 'result', 'txid').first
    return Result::Failure.new("Failed to set #{name} market order (order_id is nil)") if order_id.nil?

    data = {
      order_id: order_id
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

    client_order_id = SecureRandom.uuid
    result = client.add_order(
      cl_ord_id: client_order_id,
      ordertype: 'limit',
      type: side.to_s.downcase,
      volume: amount.to_d.to_s('F'),
      pair: ticker.ticker,
      price: price.to_d.to_s('F'),
      oflags: amount_type == :quote ? ['viqc'] : []
    )
    return result if result.failure?

    error = Utilities::Hash.dig_or_raise(result.data, 'error')
    return Result::Failure.new(*error) if error.any?

    order_id = Utilities::Hash.dig_or_raise(result.data, 'result', 'txid').first
    return Result::Failure.new("Failed to set #{name} limit order (order_id is nil)") if order_id.nil?

    data = {
      order_id: order_id
    }

    Result::Success.new(data)
  end

  def parse_order_data(order_id, order_data)
    pair = Utilities::Hash.dig_or_raise(order_data, 'descr', 'pair')
    order_type = parse_order_type(Utilities::Hash.dig_or_raise(order_data, 'descr', 'ordertype'))
    price = Utilities::Hash.dig_or_raise(order_data, 'price').to_d
    price = Utilities::Hash.dig_or_raise(order_data, 'descr', 'price').to_d if price.zero? && order_type == :limit_order
    quote_amount_exec = Utilities::Hash.dig_or_raise(order_data, 'cost').to_d
    amount_exec = Utilities::Hash.dig_or_raise(order_data, 'vol_exec').to_d

    order_flags = Utilities::Hash.dig_or_raise(order_data, 'oflags').split(',')
    if order_flags.include?('viqc')
      amount = nil
      quote_amount = Utilities::Hash.dig_or_raise(order_data, 'vol').to_d
    else
      amount = Utilities::Hash.dig_or_raise(order_data, 'vol').to_d
      quote_amount = nil
    end

    side = Utilities::Hash.dig_or_raise(order_data, 'descr', 'type').downcase.to_sym
    errors = [
      order_data['reason'].presence,
      order_data['misc'].presence
    ].compact
    status = parse_order_status(Utilities::Hash.dig_or_raise(order_data, 'status'))
    ticker = tickers.find_by(ticker: pair)

    {
      order_id: order_id,
      ticker: ticker,
      price: price,
      amount: amount,                       # amount in the order config
      quote_amount: quote_amount,           # amount in the order config
      amount_exec: amount_exec,             # amount the account balance went up or down
      quote_amount_exec: quote_amount_exec, # amount the account balance went up or down
      side: side,
      order_type: order_type,
      error_messages: errors,
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
    # pending, open, closed, canceled, expired
    case status
    when 'pending'
      :unknown
    when 'open'
      :open
    when 'closed', 'canceled', 'expired'
      :closed
    else
      raise "Unknown #{name} order status: #{status}"
    end
  end
end
