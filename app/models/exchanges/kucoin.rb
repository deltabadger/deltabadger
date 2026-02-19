class Exchanges::Kucoin < Exchange
  COINGECKO_ID = 'kucoin'.freeze # https://docs.coingecko.com/reference/exchanges-list
  ERRORS = {
    insufficient_funds: ['Balance insufficient!', 'Insufficient balance'],
    invalid_key: ['Invalid API-Key', 'Invalid KC-API-SIGN', 'Invalid passphrase']
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
    @client = Clients::Kucoin.new(
      api_key: api_key&.key,
      api_secret: api_key&.secret,
      passphrase: api_key&.passphrase
    )
  end

  def get_tickers_info(force: false)
    cache_key = "exchange_#{id}_tickers_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour, force: force) do
      result = client.get_symbols
      return result if result.failure?

      data = result.data['data']
      return Result::Failure.new("Failed to get #{name} symbols") if data.nil?

      data.map do |product|
        symbol = Utilities::Hash.dig_or_raise(product, 'symbol')
        base = Utilities::Hash.dig_or_raise(product, 'baseCurrency')
        quote = Utilities::Hash.dig_or_raise(product, 'quoteCurrency')
        enable_trading = Utilities::Hash.dig_or_raise(product, 'enableTrading')
        base_min_size = Utilities::Hash.dig_or_raise(product, 'baseMinSize').to_d
        quote_min_size = Utilities::Hash.dig_or_raise(product, 'quoteMinSize').to_d
        base_max_size = Utilities::Hash.dig_or_raise(product, 'baseMaxSize').to_d
        quote_max_size = Utilities::Hash.dig_or_raise(product, 'quoteMaxSize').to_d
        base_increment = Utilities::Hash.dig_or_raise(product, 'baseIncrement')
        quote_increment = Utilities::Hash.dig_or_raise(product, 'quoteIncrement')
        price_increment = Utilities::Hash.dig_or_raise(product, 'priceIncrement')

        {
          ticker: symbol,
          base: base,
          quote: quote,
          minimum_base_size: base_min_size,
          minimum_quote_size: quote_min_size,
          maximum_base_size: base_max_size,
          maximum_quote_size: quote_max_size,
          base_decimals: Utilities::Number.decimals(base_increment),
          quote_decimals: Utilities::Number.decimals(quote_increment),
          price_decimals: Utilities::Number.decimals(price_increment),
          available: enable_trading
        }
      end.compact
    end

    Result::Success.new(tickers_info)
  end

  def get_tickers_prices(force: false)
    cache_key = "exchange_#{id}_prices"
    tickers_prices = Rails.cache.fetch(cache_key, expires_in: 1.minute, force: force) do
      result = client.get_all_tickers
      return result if result.failure?

      data = result.data.dig('data', 'ticker')
      return Result::Failure.new("Failed to get #{name} tickers") if data.nil?

      data.each_with_object({}) do |ticker_data, prices_hash|
        symbol = Utilities::Hash.dig_or_raise(ticker_data, 'symbol')
        price = ticker_data['last']&.to_d || 0
        prices_hash[symbol] = price if price.positive?
      end
    end

    Result::Success.new(tickers_prices)
  end

  def get_balances(asset_ids: nil)
    result = client.get_accounts(type: 'trade')
    return result if result.failure?

    data = result.data['data']
    return Result::Failure.new("Failed to get #{name} balances") if data.nil?

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.each_with_object({}) do |asset_id, balances_hash|
      balances_hash[asset_id] = { free: 0, locked: 0 }
    end
    data.each do |balance|
      currency = Utilities::Hash.dig_or_raise(balance, 'currency')
      asset = asset_from_symbol(currency)
      next unless asset.present?
      next unless asset_ids.include?(asset.id)

      free = Utilities::Hash.dig_or_raise(balance, 'available').to_d
      locked = Utilities::Hash.dig_or_raise(balance, 'holds').to_d

      balances[asset.id] = { free: free, locked: locked }
    end

    Result::Success.new(balances)
  end

  def get_last_price(ticker:, force: false)
    cache_key = "exchange_#{id}_last_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      result = client.get_all_tickers
      return result if result.failure?

      data = result.data.dig('data', 'ticker')
      return Result::Failure.new("Failed to get #{name} last price") if data.nil?

      ticker_data = data.find { |t| t['symbol'] == ticker.ticker }
      return Result::Failure.new("Ticker #{ticker.ticker} not found on #{name}") if ticker_data.nil?

      price = ticker_data['last'].to_d
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
      1.minute => '1min',
      5.minutes => '5min',
      15.minutes => '15min',
      30.minutes => '30min',
      1.hour => '1hour',
      4.hours => '4hour',
      1.day => '1day',
      3.days => '1day',
      1.week => '1week',
      1.month => '1week'
    }
    interval = intervals[timeframe]

    candles = []
    loop do
      end_at = [start_at + (1500 * timeframe), Time.now.utc].min
      result = client.get_candles(
        symbol: ticker.ticker,
        type: interval,
        start_at: start_at.to_i,
        end_at: end_at.to_i
      )
      return result if result.failure?

      data = result.data['data']
      break if data.blank?

      data.sort_by { |c| c[0] }.each do |candle|
        candles << [
          Time.at(candle[0].to_i).utc,
          candle[1].to_d, # open
          candle[3].to_d, # high
          candle[4].to_d, # low
          candle[2].to_d, # close
          candle[5].to_d  # volume
        ]
      end
      break if end_at >= Time.now.utc

      start_at = candles.last[0] + 1.second
    end

    candles = build_candles_from_candles(candles: candles, timeframe: timeframe) if timeframe.in?([3.days, 1.month])

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
    return result if result.failure?

    order_data = result.data['data']
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
    temp_client = Clients::Kucoin.new(
      api_key: api_key.key,
      api_secret: api_key.secret,
      passphrase: api_key.passphrase
    )
    result = temp_client.get_accounts(type: 'trade')

    if result.success? && result.data['code'] == '200000'
      Result::Success.new(true)
    elsif result.success?
      Result::Success.new(false)
    elsif result.data.is_a?(Hash) && result.data[:status] == 401
      Result::Success.new(false)
    else
      result
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
      currencies_result = Clients::Kucoin.new.get_currencies
      return currencies_result if currencies_result.failure?

      coin_data = Array(currencies_result.data['data']).find { |c| c['currency'] == symbol }
      return Result::Failure.new("No currency data found for #{symbol} on KuCoin") if coin_data.blank?

      chains = coin_data['chains'] || []
      chain = chains.find { |c| c['isDefault'] == true } || chains.first
      chain_name = chain&.dig('chainName')
    end

    result = client.withdraw(currency: symbol, address: address, amount: amount.to_d.to_s('F'),
                             chain: chain_name, memo: address_tag)
    return result if result.failure?

    withdrawal_id = result.data.dig('data', 'withdrawalId')
    Result::Success.new({ withdrawal_id: withdrawal_id })
  end

  def fetch_withdrawal_fees!
    result = Clients::Kucoin.new.get_currencies
    return result if result.failure?

    fees = {}
    chain_data = {}
    Array(result.data['data']).each do |coin|
      symbol = coin['currency']
      coin_chains = coin['chains'] || []
      chain = coin_chains.find { |c| c['isDefault'] == true } || coin_chains.first
      next unless chain

      fees[symbol] = chain['withdrawalMinFee']
      chain_data[symbol] = coin_chains.map do |c|
        { 'name' => c['chainName'], 'fee' => c['withdrawalMinFee'], 'is_default' => c['isDefault'] == true }
      end
    end

    update_exchange_asset_fees!(fees, chains: chain_data)
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
      result = client.get_orderbook(symbol: ticker.ticker, limit: 20)
      return result if result.failure?

      data = result.data['data']
      return Result::Failure.new("Failed to get #{name} order book for #{ticker.ticker}") if data.nil?

      Result::Success.new(
        {
          bid: {
            price: data['bids'][0][0].to_d,
            size: data['bids'][0][1].to_d
          },
          ask: {
            price: data['asks'][0][0].to_d,
            size: data['asks'][0][1].to_d
          }
        }
      )
    end
  end

  # @param amount [Float] must be a positive number
  # @param amount_type [Symbol] :base or :quote
  # @param side [Symbol] must be either :buy or :sell
  def set_market_order(ticker:, amount:, amount_type:, side:)
    amount = ticker.adjusted_amount(amount: amount, amount_type: amount_type)

    order_settings = {
      client_oid: SecureRandom.uuid,
      symbol: ticker.ticker,
      side: side.to_s,
      type: 'market',
      size: amount_type == :base ? amount.to_d.to_s('F') : nil,
      funds: amount_type == :quote ? amount.to_d.to_s('F') : nil
    }
    result = client.create_order(**order_settings)
    return result if result.failure?

    return Result::Failure.new(result.data['msg'] || "Failed to set #{name} market order") if result.data['code'] != '200000'

    order_id = Utilities::Hash.dig_or_raise(result.data, 'data', 'orderId')

    Result::Success.new({ order_id: order_id })
  end

  # @param amount [Float] must be a positive number
  # @param amount_type [Symbol] :base or :quote
  # @param side [Symbol] must be either :buy or :sell
  # @param price [Float] must be a positive number
  def set_limit_order(ticker:, amount:, amount_type:, side:, price:)
    amount = ticker.adjusted_amount(amount: amount, amount_type: amount_type)
    price = ticker.adjusted_price(price: price)

    order_settings = {
      client_oid: SecureRandom.uuid,
      symbol: ticker.ticker,
      side: side.to_s,
      type: 'limit',
      price: price.to_d.to_s('F'),
      size: amount.to_d.to_s('F')
    }
    result = client.create_order(**order_settings)
    return result if result.failure?

    return Result::Failure.new(result.data['msg'] || "Failed to set #{name} limit order") if result.data['code'] != '200000'

    order_id = Utilities::Hash.dig_or_raise(result.data, 'data', 'orderId')

    Result::Success.new({ order_id: order_id })
  end

  def parse_order_data(order_id, order_data)
    symbol = Utilities::Hash.dig_or_raise(order_data, 'symbol')
    ticker = tickers.find_by(ticker: symbol)
    order_type = parse_order_type(Utilities::Hash.dig_or_raise(order_data, 'type'))
    price = if order_data['dealFunds']&.to_d&.positive? && order_data['dealSize']&.to_d&.positive?
              (order_data['dealFunds'].to_d / order_data['dealSize'].to_d)
            else
              order_data['price']&.to_d || 0
            end
    price = ticker.adjusted_price(price: price, method: :round) if price.positive? && ticker.present?
    price = nil if price.zero?
    amount = order_data['size']&.to_d
    quote_amount = order_data['funds']&.to_d
    side = Utilities::Hash.dig_or_raise(order_data, 'side').downcase.to_sym
    amount_exec = order_data['dealSize']&.to_d || 0
    quote_amount_exec = order_data['dealFunds']&.to_d || 0
    status = parse_order_status(order_data)
    errors = [order_data['cancelReason'].presence].compact

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
      error_messages: errors,
      status: status,
      exchange_response: order_data
    }
  end

  def parse_order_type(order_type)
    case order_type.downcase
    when 'market'
      :market_order
    when 'limit'
      :limit_order
    else
      raise "Unknown #{name} order type: #{order_type}"
    end
  end

  def parse_order_status(order_data)
    is_active = order_data['isActive']
    cancel_exist = order_data['cancelExist']

    if cancel_exist
      :cancelled
    elsif is_active
      :open
    else
      deal_size = order_data['dealSize']&.to_d || 0
      deal_size.positive? ? :closed : :unknown
    end
  end
end
