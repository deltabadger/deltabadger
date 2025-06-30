class Exchanges::Binance < Exchange
  COINGECKO_ID = 'binance'.freeze # https://docs.coingecko.com/reference/exchanges-list
  ERRORS = {
    insufficient_funds: ['Account has insufficient balance for requested action.'],
    invalid_key: ['API-key format invalid.', 'Invalid API-key, IP, or permissions for action.']
  }.freeze # https://developers.binance.com/docs/binance-spot-api-docs/errors
  ERROR_CODES = {
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
    @proxy_ip ||= Clients::Binance::PROXY.split('://').last.split(':').first if Clients::Binance::PROXY.present?
  end

  def set_client(api_key: nil)
    @api_key = api_key
    @client = Clients::Binance.new(
      api_key: api_key&.key,
      api_secret: api_key&.secret
    )
  end

  def get_tickers_info(force: false)
    cache_key = "exchange_#{id}_tickers_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour, force: force) do
      result = client.exchange_information(permissions: ['SPOT'])
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error) : result
      end

      result.data['symbols'].map do |product|
        ticker = Utilities::Hash.dig_or_raise(product, 'symbol')
        status = Utilities::Hash.dig_or_raise(product, 'status')

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
          quote_decimals: Utilities::Hash.dig_or_raise(product, 'quoteAssetPrecision'),
          price_decimals: Utilities::Number.decimals(price_filter['tickSize']),
          available: status == 'TRADING'
        }
      end.compact
    end

    Result::Success.new(tickers_info)
  end

  def get_tickers_prices(force: false)
    cache_key = "exchange_#{id}_prices"
    tickers_prices = Rails.cache.fetch(cache_key, expires_in: 1.minute, force: force) do
      result = client.symbol_price_ticker
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error) : result
      end

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
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.each_with_object({}) do |asset_id, balances_hash|
      balances_hash[asset_id] = { free: 0, locked: 0 }
    end
    raw_balances = Utilities::Hash.dig_or_raise(result.data, 'balances')
    raw_balances.each do |balance|
      asset = asset_from_symbol(balance['asset'])
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
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error) : result
      end

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
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error) : result
      end

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
    # Binance can assign same order id to different symbols
    symbol, ext_order_id = order_id.split('-')
    result = client.query_order(symbol: symbol, order_id: ext_order_id)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

    normalized_order_data = parse_order_data(order_id, result.data)
    if normalized_order_data[:amount_exec_excl_commission].zero?
      normalized_order_data[:amount_exec] = normalized_order_data.delete(:amount_exec_excl_commission)
      normalized_order_data[:quote_amount_exec] = normalized_order_data.delete(:quote_amount_exec_excl_commission)
    else
      # gather real amount_exec, quote_amount_exec, price from trades (which includes commission)
      result = get_aggregated_trades_for_orders([normalized_order_data])
      return result if result.failure?

      aggregated_trades_data = result.data[normalized_order_data[:order_id]]
      normalized_order_data[:price] = aggregated_trades_data[:price]
      normalized_order_data[:amount_exec] = aggregated_trades_data[:amount_exec]
      normalized_order_data[:quote_amount_exec] = aggregated_trades_data[:quote_amount_exec]
      normalized_order_data.delete(:amount_exec_excl_commission)
      normalized_order_data.delete(:quote_amount_exec_excl_commission)
    end

    Result::Success.new(normalized_order_data)
  end

  def get_orders(order_ids:)
    orders = {}
    orders_with_trades = []
    order_ids_by_symbol = order_ids.group_by { |order_id| order_id.split('-').first }
    order_ids_by_symbol.each do |symbol, symbol_order_ids|
      ext_order_ids = symbol_order_ids.map { |order_id| order_id.split('-').last.to_i }
      100.times do |i|
        raise "Too many attempts to get #{name} orders. Adjust the number of iterations in the loop if needed." if i == 100

        result = client.all_orders(symbol: symbol, order_id: ext_order_ids.min, limit: 1000)
        if result.failure?
          error = parse_error_message(result)
          return error.present? ? Result::Failure.new(error) : result
        end

        result.data.each do |order_data|
          ext_order_id = Utilities::Hash.dig_or_raise(order_data, 'orderId')
          next unless ext_order_id.in?(ext_order_ids)

          order_id = "#{symbol}-#{ext_order_id}"

          normalized_order_data = parse_order_data(order_id, order_data)
          if normalized_order_data[:amount_exec_excl_commission].zero?
            normalized_order_data[:amount_exec] = normalized_order_data.delete(:amount_exec_excl_commission)
            normalized_order_data[:quote_amount_exec] = normalized_order_data.delete(:quote_amount_exec_excl_commission)
          else
            orders_with_trades << normalized_order_data
          end

          orders[order_id] = normalized_order_data
          ext_order_ids.delete(ext_order_id)
        end

        break if ext_order_ids.empty?
      end
    end

    if orders_with_trades.any?
      result = get_aggregated_trades_for_orders(orders_with_trades)
      return result if result.failure?

      aggregated_trades_datas = result.data
      aggregated_trades_datas.each do |order_id, aggregated_trades_data|
        orders[order_id][:price] = aggregated_trades_data[:price]
        orders[order_id][:amount_exec] = aggregated_trades_data[:amount_exec]
        orders[order_id][:quote_amount_exec] = aggregated_trades_data[:quote_amount_exec]
        orders[order_id].delete(:amount_exec_excl_commission)
        orders[order_id].delete(:quote_amount_exec_excl_commission)
      end
    end

    Result::Success.new(orders)
  end

  def get_aggregated_trades_for_orders(orders)
    aggregated_trades_by_order_id = {}
    symbols = orders.group_by { |order| order[:order_id].split('-').first }
    limit = 1000
    symbols.each do |symbol, symbol_orders|
      ext_order_ids = symbol_orders.map { |order| order[:order_id].split('-').last.to_i }
      if ext_order_ids.count < 5 # request weight is 5 when passing one order id
        ext_order_ids.each do |ext_order_id|
          # we assume one order will never have more than 1000 trades
          result = client.account_trade_list(symbol: symbol, order_id: ext_order_id, limit: limit)
          if result.failure?
            error = parse_error_message(result)
            return error.present? ? Result::Failure.new(error) : result
          end

          trade_datas = result.data.map { |trade_data| parse_trade_data(trade_data) }
          aggregated_trades = aggregate_trades(trade_datas)
          aggregated_trades_by_order_id[aggregated_trades[:order_id]] = aggregated_trades
        end
      else # request weight is 20 when passing no order id
        end_time = Time.current.to_i * 1000
        100.times do |i|
          if i == 100
            raise "Too many attempts to get #{name} #{symbol} trades. Adjust the number of iterations in the loop if needed."
          end

          result = client.account_trade_list(symbol: symbol, end_time: end_time, limit: limit)
          if result.failure?
            error = parse_error_message(result)
            return error.present? ? Result::Failure.new(error) : result
          end

          trade_datas = result.data.map do |raw_trade|
            parse_trade_data(raw_trade) if raw_trade['orderId'].in?(ext_order_ids)
          end.compact
          trade_datas.each do |trade_data|
            order_id = trade_data[:order_id]
            aggregated_trades_by_order_id[order_id] = aggregate_trades(
              [aggregated_trades_by_order_id[order_id], trade_data].compact
            )
          end
          break if result.data.count < limit
          break if symbol_orders.map { |o| o[:amount] == aggregated_trades_by_order_id[o[:order_id]]&.[](:amount) }.all?

          end_time = result.data.map { |raw_trade| raw_trade['time'] }.min
        end
      end
    end

    Result::Success.new(aggregated_trades_by_order_id)
  end

  def cancel_order(order_id:)
    # Binance can assign same order id to different symbols
    symbol, ext_order_id = order_id.split('-')
    result = client.cancel_order(symbol: symbol, order_id: ext_order_id)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

    Result::Success.new(order_id)
  end

  def get_api_key_validity(api_key:) # rubocop:disable Metrics/PerceivedComplexity,Metrics/CyclomaticComplexity
    result = Clients::Binance.new(
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
    elsif parse_error_code(result).in?(ERROR_CODES[:invalid_key])
      Result::Success.new(false)
    else
      result
    end
  end

  def minimum_amount_logic(order_type:, **)
    if order_type == :market_order
      :base_and_quote
    else
      :base_and_quote_in_base
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
      result = client.symbol_order_book_ticker(symbol: ticker.ticker)
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error) : result
      end

      formatted_symbol_order_book_ticker = {
        bid: {
          price: Utilities::Hash.dig_or_raise(result.data, 'bidPrice').to_d,
          size: Utilities::Hash.dig_or_raise(result.data, 'bidQty').to_d
        },
        ask: {
          price: Utilities::Hash.dig_or_raise(result.data, 'askPrice').to_d,
          size: Utilities::Hash.dig_or_raise(result.data, 'askQty').to_d
        }
      }
      Result::Success.new(formatted_symbol_order_book_ticker)
    end
  end

  # @param amount: Float must be a positive number
  # @param amount_type [Symbol] :base or :quote
  # @param side: String must be either 'buy' or 'sell'
  def set_market_order(ticker:, amount:, amount_type:, side:)
    amount = ticker.adjusted_amount(amount: amount, amount_type: amount_type)

    order_settings = {
      symbol: ticker.ticker,
      side: side.to_s.upcase,
      type: 'MARKET',
      quote_order_qty: amount_type == :quote ? amount.to_d.to_s('F') : nil,
      quantity: amount_type == :base ? amount.to_d.to_s('F') : nil,
      self_trade_prevention_mode: 'EXPIRE_MAKER'
    }
    result = client.new_order(**order_settings)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

    # Binance can assign same order id to different symbols
    ext_order_id = Utilities::Hash.dig_or_raise(result.data, 'orderId')
    data = {
      order_id: "#{ticker.ticker}-#{ext_order_id}"
    }

    Result::Success.new(data)
  end

  # @param amount: Float must be a positive number
  # @param amount_type [Symbol] :base or :quote
  # @param side: String must be either 'buy' or 'sell'
  # @param price: Float must be a positive number
  def set_limit_order(ticker:, amount:, amount_type:, side:, price:)
    amount = ticker.adjusted_amount(amount: amount, amount_type: amount_type)
    price = ticker.adjusted_price(price: price)

    order_settings = {
      symbol: ticker.ticker,
      side: side.to_s.upcase,
      type: 'LIMIT',
      time_in_force: 'GTC',
      price: price.to_d.to_s('F'),
      quantity: amount_type == :base ? amount.to_d.to_s('F') : nil
    }
    result = client.new_order(**order_settings)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

    # Binance can assign same order id to different symbols
    ext_order_id = Utilities::Hash.dig_or_raise(result.data, 'orderId')
    data = {
      order_id: "#{ticker.ticker}-#{ext_order_id}"
    }

    Result::Success.new(data)
  end

  def parse_order_data(order_id, order_data)
    symbol = Utilities::Hash.dig_or_raise(order_data, 'symbol')
    ticker = tickers.find_by(ticker: symbol)
    order_type = parse_order_type(Utilities::Hash.dig_or_raise(order_data, 'type'))
    amount = Utilities::Hash.dig_or_raise(order_data, 'origQty').to_d
    amount = amount.zero? ? nil : amount
    quote_amount = Utilities::Hash.dig_or_raise(order_data, 'origQuoteOrderQty').to_d
    quote_amount = quote_amount.zero? ? nil : quote_amount
    side = Utilities::Hash.dig_or_raise(order_data, 'side').downcase.to_sym
    amount_exec_excl_commission = Utilities::Hash.dig_or_raise(order_data, 'executedQty').to_d
    quote_amount_exec_excl_commission = Utilities::Hash.dig_or_raise(order_data, 'cummulativeQuoteQty').to_d
    quote_amount_exec_excl_commission = quote_amount_exec_excl_commission.negative? ? nil : quote_amount_exec_excl_commission # for some historical orders
    status = parse_order_status(Utilities::Hash.dig_or_raise(order_data, 'status'))
    price = Utilities::Hash.dig_or_raise(order_data, 'price').to_d
    if price.zero? &&
       quote_amount_exec_excl_commission.present? &&
       quote_amount_exec_excl_commission.positive? &&
       amount_exec_excl_commission.positive?
      price = quote_amount_exec_excl_commission / amount_exec_excl_commission
      price = ticker.adjusted_price(price: price, method: :round) if ticker.present?
    end
    price = nil if price.zero?

    {
      order_id: order_id,
      ticker: ticker,
      price: price,
      amount: amount,                       # amount in the order config
      quote_amount: quote_amount,           # amount in the order config
      amount_exec_excl_commission: amount_exec_excl_commission,
      quote_amount_exec_excl_commission: quote_amount_exec_excl_commission,
      side: side,
      order_type: order_type,
      error_messages: [],
      status: status,
      exchange_response: order_data
    }
  end

  def parse_trade_data(trade_data)
    symbol = Utilities::Hash.dig_or_raise(trade_data, 'symbol')
    order_id = "#{symbol}-#{Utilities::Hash.dig_or_raise(trade_data, 'orderId')}"
    ticker = tickers.find_by(ticker: symbol)
    price = Utilities::Hash.dig_or_raise(trade_data, 'price').to_d
    amount = Utilities::Hash.dig_or_raise(trade_data, 'qty').to_d
    quote_amount = Utilities::Hash.dig_or_raise(trade_data, 'quoteQty').to_d
    commission = Utilities::Hash.dig_or_raise(trade_data, 'commission').to_d
    commission_asset = Utilities::Hash.dig_or_raise(trade_data, 'commissionAsset')
    side = Utilities::Hash.dig_or_raise(trade_data, 'isBuyer') == true ? :buy : :sell
    # Fallback to symbol if ticker is not found, only used for renamed tickers like RNDRUSDT -> RENDERUSDC
    commission_in_base = ticker.present? ? commission_asset == ticker.base : symbol.start_with?(commission_asset)
    commission_in_quote = ticker.present? ? commission_asset == ticker.quote : symbol.end_with?(commission_asset)
    amount_exec = if commission_in_base
                    side == :buy ? (amount - commission) : (amount + commission)
                  else
                    amount
                  end
    quote_amount_exec = if commission_in_quote
                          side == :buy ? (quote_amount + commission) : (quote_amount - commission)
                        else
                          quote_amount
                        end
    order_type = Utilities::Hash.dig_or_raise(trade_data, 'isMaker') == true ? :market_order : :limit_order

    {
      order_id: order_id,
      ticker: ticker,
      price: price,
      amount: amount,
      quote_amount: quote_amount,
      amount_exec: amount_exec,             # amount the account balance went up or down
      quote_amount_exec: quote_amount_exec, # amount the account balance went up or down
      side: side,
      order_type: order_type,
      exchange_response: trade_data
    }
  end

  def aggregate_trades(trade_datas)
    prices = []
    amounts = []
    quote_amounts = []
    amount_execs = []
    quote_amount_execs = []
    trade_datas.each do |trade_data|
      prices << trade_data[:price]
      amounts << trade_data[:amount]
      quote_amounts << trade_data[:quote_amount]
      amount_execs << trade_data[:amount_exec]
      quote_amount_execs << trade_data[:quote_amount_exec]
    end
    ticker = trade_datas.first[:ticker]
    price = Utilities::Math.weighted_average(prices, amounts)
    price = ticker.adjusted_price(price: price, method: :round) if ticker.present?
    {
      order_id: trade_datas.first[:order_id],
      ticker: ticker,
      price: price,
      amount: amounts.sum,
      quote_amount: quote_amounts.sum,
      amount_exec: amount_execs.sum,             # amount the account balance went up or down
      quote_amount_exec: quote_amount_execs.sum, # amount the account balance went up or down
      side: trade_datas.first[:side],
      order_type: trade_datas.first[:order_type],
      exchange_response: nil
    }
  end

  def parse_order_type(order_type)
    case order_type
    when 'MARKET'
      :market_order
    when 'LIMIT'
      :limit_order
    else
      raise "Unknown #{name} order type: #{order_type}"
    end
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
