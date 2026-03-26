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

  def get_tickers_prices(force: false, symbols: nil)
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
    balances = asset_ids.to_h do |asset_id|
      [asset_id, { free: 0, locked: 0 }]
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
      break if result.data.last.nil? || result.data.empty? || result.data.last[0] > timeframe.ago.to_i * 1000

      start_at = candles.empty? ? start_at + (limit * interval.to_i * 1000) : candles.last[0] + 1
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
          raise "Too many attempts to get #{name} #{symbol} trades. Adjust the number of iterations in the loop if needed." if i == 100

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

  def get_api_key_validity(api_key:)
    result = Clients::Binance.new(
      api_key: api_key.key,
      api_secret: api_key.secret
    ).api_description

    if result.success?
      common_checks = result.data['ipRestrict'] == true &&
                      result.data['enableFixApiTrade'] == false &&
                      result.data['enableFixReadOnly'] == false &&
                      result.data['enableFutures'] == false &&
                      result.data['enableInternalTransfer'] == false &&
                      result.data['enableMargin'] == false &&
                      result.data['enablePortfolioMarginTrading'] == false &&
                      result.data['enableReading'] == true &&
                      result.data['enableVanillaOptions'] == false &&
                      result.data['permitsUniversalTransfer'] == false

      valid = if api_key.withdrawal?
                common_checks &&
                  result.data['enableWithdrawals'] == true &&
                  result.data['enableSpotAndMarginTrading'] == false
              else
                common_checks &&
                  result.data['enableSpotAndMarginTrading'] == true &&
                  result.data['enableWithdrawals'] == false
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

  FIAT_CURRENCIES = %w[USD EUR GBP AUD CAD JPY CHF TRY BRL PLN UAH CZK SEK NOK DKK HUF RON BGN ZAR NGN KES].freeze

  def get_ledger(api_key:, start_time: nil)
    hm_client = honeymaker_client(api_key)
    start_ms = start_time ? (start_time.to_f * 1000).to_i : nil
    entries = []

    # Deposits
    result = hm_client.deposit_history(start_time: start_ms)
    return result if result.failure?

    Array(result.data).each do |dep|
      next unless dep['status'] == 1

      entries << {
        entry_type: :deposit, base_currency: dep['coin'], base_amount: dep['amount'].to_d,
        quote_currency: nil, quote_amount: nil, fee_currency: nil, fee_amount: nil,
        tx_id: dep['txId'], group_id: nil, description: nil,
        transacted_at: Time.at(dep['insertTime'] / 1000.0).utc, raw_data: dep
      }
    end

    # Withdrawals
    result = hm_client.withdraw_history(start_time: start_ms)
    return result if result.failure?

    Array(result.data).each do |wd|
      next unless wd['status'] == 6

      entries << {
        entry_type: :withdrawal, base_currency: wd['coin'], base_amount: wd['amount'].to_d,
        quote_currency: nil, quote_amount: nil,
        fee_currency: wd['coin'], fee_amount: wd['transactionFee']&.to_d,
        tx_id: wd['txId'] || wd['id'], group_id: nil, description: nil,
        transacted_at: Time.parse(wd['applyTime']).utc, raw_data: wd
      }
    end

    # Discover traded symbols and fetch spot trades
    if start_time
      traded_symbols = known_traded_symbols(api_key)
    else
      traded_coins = discover_traded_coins(hm_client, entries)
      symbols_map = load_exchange_symbols(hm_client)
      traded_symbols = symbols_map.keys.select { |sym| traded_coins.include?(symbols_map[sym][:base]) }
    end

    traded_symbols.each do |symbol|
      result = hm_client.account_trade_list(symbol: symbol, start_time: start_ms)
      next if result.failure?

      Array(result.data).each do |trade|
        entries.concat(normalize_trade(trade, symbol))
      end
    end

    # Convert trades (30-day windows)
    import_convert_trades(hm_client, start_ms, entries)

    # Fiat buy/sell
    import_fiat_payments(hm_client, start_ms, entries)

    # Dust conversions
    import_dust_conversions(hm_client, start_ms, entries)

    # Dividends / airdrops
    import_dividends(hm_client, start_ms, entries)

    # Earn rewards
    import_earn_rewards(hm_client, start_ms, entries)

    # Margin interest
    import_margin_interest(hm_client, start_ms, entries)

    # Margin liquidations
    import_margin_liquidations(hm_client, start_ms, entries)

    # Futures income (PNL, funding fees, commissions)
    import_futures_income(hm_client, start_ms, entries)

    # Earn subscriptions/redemptions
    import_earn_subscriptions(hm_client, start_ms, entries)

    Result::Success.new(entries)
  end

  def list_withdrawal_addresses(asset:)
    symbol = symbol_from_asset(asset)
    return nil if symbol.blank?

    result = client.get_withdraw_addresses
    return nil if result.failure?

    addresses = Array(result.data)
    addresses.filter_map do |addr|
      next unless addr['coin'] == symbol

      address = addr['address']
      label_parts = [address, addr['name'], addr['network']].compact_blank
      { name: address, label: label_parts.join(' - ') }
    end
  end

  def withdraw(asset:, amount:, address:, network: nil, address_tag: nil)
    symbol = symbol_from_asset(asset)
    return Result::Failure.new("Unknown symbol for asset #{asset.symbol}") if symbol.blank?

    result = client.withdraw(coin: symbol, address: address, amount: amount.to_d.to_s('F'),
                             network: network, address_tag: address_tag)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

    withdrawal_id = result.data['id']
    Result::Success.new({ withdrawal_id: withdrawal_id })
  end

  def fetch_withdrawal_fees!
    api_key = fee_api_key
    return Result::Success.new({}) if api_key.blank?

    result = Clients::Binance.new(
      api_key: api_key.key,
      api_secret: api_key.secret
    ).get_all_coins_information
    return result if result.failure?

    fees = {}
    chains = {}
    result.data.each do |coin|
      symbol = coin['coin']
      networks = coin['networkList'] || []
      network = networks.find { |n| n['isDefault'] == true } || networks.first
      next unless network

      fees[symbol] = network['withdrawFee']
      chains[symbol] = networks.map do |n|
        { 'name' => n['network'], 'fee' => n['withdrawFee'], 'is_default' => n['isDefault'] == true }
      end
    end

    update_exchange_asset_fees!(fees, chains: chains)
  end

  private

  def client
    @client ||= set_client
  end

  def honeymaker_client(api_key)
    Honeymaker.client('binance',
                      api_key: api_key.key,
                      api_secret: api_key.secret,
                      proxy: ENV['PROXY_BINANCE'])
  end

  def normalize_trade(trade, symbol)
    base_asset_symbol = symbol_pair_base(symbol)
    quote_asset_symbol = symbol_pair_quote(symbol)
    is_buyer = trade['isBuyer']
    is_fiat_quote = FIAT_CURRENCIES.include?(quote_asset_symbol) || quote_asset_symbol.match?(/^USD[TC]?$|^BUSD$|^DAI$|^FDUSD$/)
    trade_id = "#{symbol}-#{trade['orderId']}-#{trade['id']}"
    transacted_at = Time.at(trade['time'] / 1000.0).utc

    if is_fiat_quote
      [{
        entry_type: is_buyer ? :buy : :sell,
        base_currency: base_asset_symbol,
        base_amount: trade['qty'].to_d,
        quote_currency: quote_asset_symbol,
        quote_amount: trade['quoteQty'].to_d,
        fee_currency: trade['commissionAsset'],
        fee_amount: trade['commission'].to_d,
        tx_id: trade_id,
        group_id: nil,
        description: nil,
        transacted_at: transacted_at,
        raw_data: trade
      }]
    else
      group_id = "swap_#{trade_id}"
      [
        {
          entry_type: is_buyer ? :swap_in : :swap_out,
          base_currency: base_asset_symbol,
          base_amount: trade['qty'].to_d,
          quote_currency: nil,
          quote_amount: nil,
          fee_currency: is_buyer ? trade['commissionAsset'] : nil,
          fee_amount: is_buyer ? trade['commission'].to_d : nil,
          tx_id: "#{trade_id}-in",
          group_id: group_id,
          description: nil,
          transacted_at: transacted_at,
          raw_data: trade
        },
        {
          entry_type: is_buyer ? :swap_out : :swap_in,
          base_currency: quote_asset_symbol,
          base_amount: trade['quoteQty'].to_d,
          quote_currency: nil,
          quote_amount: nil,
          fee_currency: is_buyer ? nil : trade['commissionAsset'],
          fee_amount: is_buyer ? nil : trade['commission'].to_d,
          tx_id: "#{trade_id}-out",
          group_id: group_id,
          description: nil,
          transacted_at: transacted_at,
          raw_data: trade
        }
      ]
    end
  end

  def symbol_pair_base(symbol)
    @exchange_symbols_map&.dig(symbol, :base) || tickers.find_by(ticker: symbol)&.base || symbol
  end

  def symbol_pair_quote(symbol)
    @exchange_symbols_map&.dig(symbol, :quote) || tickers.find_by(ticker: symbol)&.quote || symbol
  end

  def known_traded_symbols(api_key)
    AccountTransaction.where(api_key: api_key)
                      .where.not(quote_currency: nil)
                      .where(entry_type: %i[buy sell swap_in swap_out])
                      .distinct
                      .pluck(:base_currency, :quote_currency)
                      .map { |base, quote| "#{base}#{quote}" }
                      .uniq
  end

  def discover_traded_coins(hm_client, entries)
    coins = Set.new
    entries.each { |e| coins << e[:base_currency] }
    result = hm_client.account_information(omit_zero_balances: true)
    if result.success?
      Array(result.data['balances']).each do |bal|
        coins << bal['asset'] if bal['free'].to_d.positive? || bal['locked'].to_d.positive?
      end
    end
    coins
  end

  def load_exchange_symbols(hm_client)
    @exchange_symbols_map = {}
    result = hm_client.exchange_information
    return @exchange_symbols_map if result.failure?

    Array(result.data['symbols']).each do |s|
      @exchange_symbols_map[s['symbol']] = { base: s['baseAsset'], quote: s['quoteAsset'] }
    end
    @exchange_symbols_map
  end

  def import_convert_trades(hm_client, start_ms, entries)
    # Convert API requires 30-day windows
    window_start = start_ms || (Time.utc(2020, 1, 1).to_f * 1000).to_i
    window_end = (Time.now.utc.to_f * 1000).to_i
    thirty_days = 30 * 24 * 60 * 60 * 1000

    while window_start < window_end
      chunk_end = [window_start + thirty_days, window_end].min
      result = hm_client.convert_trade_flow(start_time: window_start, end_time: chunk_end)
      break if result.failure?

      Array(result.data['list']).each do |conv|
        group_id = "convert_#{conv['quoteId']}"
        transacted_at = Time.at(conv['createTime'].to_i / 1000.0).utc
        entries << {
          entry_type: :swap_out, base_currency: conv['fromAsset'], base_amount: conv['fromAmount'].to_d,
          quote_currency: nil, quote_amount: nil, fee_currency: nil, fee_amount: nil,
          tx_id: "#{conv['quoteId']}-out", group_id: group_id, description: 'Convert',
          transacted_at: transacted_at, raw_data: conv
        }
        entries << {
          entry_type: :swap_in, base_currency: conv['toAsset'], base_amount: conv['toAmount'].to_d,
          quote_currency: nil, quote_amount: nil, fee_currency: nil, fee_amount: nil,
          tx_id: "#{conv['quoteId']}-in", group_id: group_id, description: 'Convert',
          transacted_at: transacted_at, raw_data: conv
        }
      end
      window_start = chunk_end
    end
  end

  def import_fiat_payments(hm_client, start_ms, entries)
    # Buy (type 0)
    result = hm_client.fiat_payments(transaction_type: 0, begin_time: start_ms)
    if result.success?
      Array(result.data['data']).each do |pay|
        next unless pay['status'] == 'Completed'

        entries << {
          entry_type: :buy, base_currency: pay['cryptoCurrency'], base_amount: pay['obtainAmount'].to_d,
          quote_currency: pay['fiatCurrency'], quote_amount: pay['sourceAmount'].to_d,
          fee_currency: pay['fiatCurrency'], fee_amount: pay['totalFee'].to_d,
          tx_id: "fiat-buy-#{pay['orderNo']}", group_id: nil, description: 'Fiat purchase',
          transacted_at: Time.at(pay['createTime'].to_i / 1000.0).utc, raw_data: pay
        }
      end
    end

    # Sell (type 1)
    result = hm_client.fiat_payments(transaction_type: 1, begin_time: start_ms)
    return unless result.success?

    Array(result.data['data']).each do |pay|
      next unless pay['status'] == 'Completed'

      entries << {
        entry_type: :sell, base_currency: pay['cryptoCurrency'], base_amount: pay['sourceAmount'].to_d,
        quote_currency: pay['fiatCurrency'], quote_amount: pay['obtainAmount'].to_d,
        fee_currency: pay['fiatCurrency'], fee_amount: pay['totalFee'].to_d,
        tx_id: "fiat-sell-#{pay['orderNo']}", group_id: nil, description: 'Fiat sale',
        transacted_at: Time.at(pay['createTime'].to_i / 1000.0).utc, raw_data: pay
      }
    end
  end

  def import_dust_conversions(hm_client, start_ms, entries)
    result = hm_client.dust_log(start_time: start_ms)
    return unless result.success?

    Array(result.data['userAssetDribblets']).each do |dribblet|
      transacted_at = Time.at(dribblet['operateTime'].to_i / 1000.0).utc
      Array(dribblet['userAssetDribbletDetails']).each do |detail|
        group_id = "dust_#{detail['transId']}"
        entries << {
          entry_type: :swap_out, base_currency: detail['fromAsset'],
          base_amount: detail['amount'].to_d,
          quote_currency: nil, quote_amount: nil,
          fee_currency: nil, fee_amount: nil,
          tx_id: "dust-#{detail['transId']}-out", group_id: group_id, description: 'Dust conversion',
          transacted_at: transacted_at, raw_data: detail
        }
        entries << {
          entry_type: :swap_in, base_currency: 'BNB',
          base_amount: detail['transferedAmount'].to_d,
          quote_currency: nil, quote_amount: nil,
          fee_currency: 'BNB', fee_amount: detail['serviceChargeAmount'].to_d,
          tx_id: "dust-#{detail['transId']}-in", group_id: group_id, description: 'Dust conversion',
          transacted_at: transacted_at, raw_data: detail
        }
      end
    end
  end

  def import_dividends(hm_client, start_ms, entries)
    result = hm_client.asset_dividend(start_time: start_ms)
    return unless result.success?

    Array(result.data['rows']).each do |div|
      entries << {
        entry_type: :airdrop, base_currency: div['asset'], base_amount: div['amount'].to_d,
        quote_currency: nil, quote_amount: nil, fee_currency: nil, fee_amount: nil,
        tx_id: "dividend-#{div['id']}", group_id: nil, description: div['enInfo'],
        transacted_at: Time.at(div['divTime'].to_i / 1000.0).utc, raw_data: div
      }
    end
  end

  def import_earn_rewards(hm_client, start_ms, entries)
    [
      [:simple_earn_flexible_rewards, 'flex-reward'],
      [:simple_earn_locked_rewards, 'lock-reward']
    ].each do |method, prefix|
      page = 1
      loop do
        result = hm_client.send(method, start_time: start_ms, current: page, size: 100)
        break if result.failure?

        rows = result.data['rows'] || []
        break if rows.empty?

        rows.each do |row|
          entries << {
            entry_type: :staking_reward, base_currency: row['asset'], base_amount: row['rewards'].to_d,
            quote_currency: nil, quote_amount: nil, fee_currency: nil, fee_amount: nil,
            tx_id: "#{prefix}-#{row['time'] || row['id']}-#{row['asset']}", group_id: nil,
            description: 'Earn reward',
            transacted_at: Time.at((row['time'] || row['deliverDate']).to_i / 1000.0).utc, raw_data: row
          }
        end

        total = result.data['total'].to_i
        break if page * 100 >= total

        page += 1
      end
    end
  end

  def import_margin_interest(hm_client, start_ms, entries)
    result = hm_client.margin_interest_history(start_time: start_ms)
    return unless result.success?

    Array(result.data['rows']).each do |row|
      entries << {
        entry_type: :fee, base_currency: row['asset'], base_amount: row['interest'].to_d,
        quote_currency: nil, quote_amount: nil, fee_currency: nil, fee_amount: nil,
        tx_id: "margin-interest-#{row['interestAccuredTime']}-#{row['asset']}",
        group_id: nil, description: 'Margin interest',
        transacted_at: Time.at(row['interestAccuredTime'].to_i / 1000.0).utc, raw_data: row
      }
    end
  end

  def import_margin_liquidations(hm_client, start_ms, entries)
    result = hm_client.margin_force_liquidation(start_time: start_ms)
    return unless result.success?

    Array(result.data['rows']).each do |row|
      entries << {
        entry_type: :sell, base_currency: row['asset'] || row['symbol'],
        base_amount: row['qty'].to_d,
        quote_currency: row['quoteAsset'], quote_amount: row['quoteQty']&.to_d,
        fee_currency: nil, fee_amount: nil,
        tx_id: "liquidation-#{row['orderId']}", group_id: nil, description: 'Liquidation',
        transacted_at: Time.at(row['updatedTime'].to_i / 1000.0).utc, raw_data: row
      }
    end
  end

  def import_futures_income(hm_client, start_ms, entries)
    # USDT-M futures
    import_futures_income_from(hm_client, :futures_income_history, 'usdt-futures', start_ms, entries)
    # COIN-M futures
    import_futures_income_from(hm_client, :coin_futures_income_history, 'coin-futures', start_ms, entries)
  end

  def import_futures_income_from(hm_client, method, prefix, start_ms, entries)
    result = hm_client.send(method, start_time: start_ms)
    return unless result.success?

    Array(result.data).each do |row|
      income_type = row['incomeType']
      amount = row['income'].to_d
      asset = row['asset']

      entry_type = case income_type
                   when 'REALIZED_PNL'
                     amount.positive? ? :other_income : :fee
                   when 'FUNDING_FEE'
                     amount.positive? ? :other_income : :fee
                   when 'COMMISSION'
                     :fee
                   else
                     next
                   end

      entries << {
        entry_type: entry_type, base_currency: asset, base_amount: amount.abs,
        quote_currency: nil, quote_amount: nil, fee_currency: nil, fee_amount: nil,
        tx_id: "#{prefix}-#{row['tranId'] || row['time']}", group_id: nil,
        description: "Futures #{income_type.downcase.tr('_', ' ')}",
        transacted_at: Time.at(row['time'].to_i / 1000.0).utc, raw_data: row
      }
    end
  end

  def import_earn_subscriptions(hm_client, start_ms, entries)
    # Subscriptions = locking funds (withdrawal from spot perspective)
    # Redemptions = unlocking funds (deposit back)
    [
      [:simple_earn_flexible_subscriptions, :withdrawal, 'flex-sub', 'purchaseAmount'],
      [:simple_earn_flexible_redemptions, :deposit, 'flex-redeem', 'amount'],
      [:simple_earn_locked_subscriptions, :withdrawal, 'lock-sub', 'purchaseAmount'],
      [:simple_earn_locked_redemptions, :deposit, 'lock-redeem', 'amount']
    ].each do |method, entry_type, prefix, amount_field|
      page = 1
      loop do
        result = hm_client.send(method, start_time: start_ms, current: page, size: 100)
        break if result.failure?

        rows = result.data['rows'] || []
        break if rows.empty?

        rows.each do |row|
          entries << {
            entry_type: entry_type, base_currency: row['asset'], base_amount: row[amount_field].to_d,
            quote_currency: nil, quote_amount: nil, fee_currency: nil, fee_amount: nil,
            tx_id: "#{prefix}-#{row['purchaseId'] || row['redeemId'] || row['time']}", group_id: nil,
            description: "Earn #{prefix.tr('-', ' ')}",
            transacted_at: Time.at((row['time'] || row['createTime']).to_i / 1000.0).utc, raw_data: row
          }
        end

        total = result.data['total'].to_i
        break if page * 100 >= total

        page += 1
      end
    end
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
    amount = nil if amount.zero?
    quote_amount = Utilities::Hash.dig_or_raise(order_data, 'origQuoteOrderQty').to_d
    quote_amount = nil if quote_amount.zero?
    side = Utilities::Hash.dig_or_raise(order_data, 'side').downcase.to_sym
    amount_exec_excl_commission = Utilities::Hash.dig_or_raise(order_data, 'executedQty').to_d
    quote_amount_exec_excl_commission = Utilities::Hash.dig_or_raise(order_data, 'cummulativeQuoteQty').to_d
    quote_amount_exec_excl_commission = nil if quote_amount_exec_excl_commission.negative? # for some historical orders
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
    when 'FILLED'
      :closed
    when 'CANCELED', 'EXPIRED', 'EXPIRED_IN_MATCH'
      :cancelled
    when 'REJECTED'
      :failed # Warning! This is not a valid external_status.
    else
      raise "Unknown #{name} order status: #{status}"
    end
  end
end
