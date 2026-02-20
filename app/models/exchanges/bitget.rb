class Exchanges::Bitget < Exchange
  COINGECKO_ID = 'bitget'.freeze # https://docs.coingecko.com/reference/exchanges-list
  ERRORS = {
    insufficient_funds: ['Insufficient balance', 'Insufficient account balance'],
    invalid_key: ['Invalid Api Key', 'Invalid ACCESS_KEY', 'Invalid signature']
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
    @client = Clients::Bitget.new(
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
        base = Utilities::Hash.dig_or_raise(product, 'baseCoin')
        quote = Utilities::Hash.dig_or_raise(product, 'quoteCoin')
        status = Utilities::Hash.dig_or_raise(product, 'status')
        min_trade_amount = Utilities::Hash.dig_or_raise(product, 'minTradeAmount').to_d
        min_trade_usd = product['minTradeUSDT']&.to_d || 0
        max_trade_amount = product['maxTradeAmount']&.to_d
        quantity_precision = Utilities::Hash.dig_or_raise(product, 'quantityPrecision').to_i
        quote_precision = Utilities::Hash.dig_or_raise(product, 'quotePrecision').to_i
        price_precision = Utilities::Hash.dig_or_raise(product, 'pricePrecision').to_i

        {
          ticker: symbol,
          base: base,
          quote: quote,
          minimum_base_size: min_trade_amount,
          minimum_quote_size: min_trade_usd,
          maximum_base_size: max_trade_amount,
          maximum_quote_size: nil,
          base_decimals: quantity_precision,
          quote_decimals: quote_precision,
          price_decimals: price_precision,
          available: status == 'online'
        }
      end.compact
    end

    Result::Success.new(tickers_info)
  end

  def get_tickers_prices(force: false)
    cache_key = "exchange_#{id}_prices"
    tickers_prices = Rails.cache.fetch(cache_key, expires_in: 1.minute, force: force) do
      result = client.get_tickers
      return result if result.failure?

      data = result.data['data']
      return Result::Failure.new("Failed to get #{name} tickers") if data.nil?

      data.each_with_object({}) do |ticker_data, prices_hash|
        symbol = Utilities::Hash.dig_or_raise(ticker_data, 'symbol')
        price = Utilities::Hash.dig_or_raise(ticker_data, 'lastPr').to_d
        prices_hash[symbol] = price
      end
    end

    Result::Success.new(tickers_prices)
  end

  def get_balances(asset_ids: nil)
    result = client.get_assets
    return result if result.failure?

    data = result.data['data']
    return Result::Failure.new("Failed to get #{name} balances") if data.nil?

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.each_with_object({}) do |asset_id, balances_hash|
      balances_hash[asset_id] = { free: 0, locked: 0 }
    end
    data.each do |balance|
      coin = Utilities::Hash.dig_or_raise(balance, 'coin')
      asset = asset_from_symbol(coin)
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
      result = client.get_tickers(symbol: ticker.ticker)
      return result if result.failure?

      data = result.data['data']
      return Result::Failure.new("Failed to get #{name} last price for #{ticker.ticker}") if data.blank?

      price = Utilities::Hash.dig_or_raise(data.first, 'lastPr').to_d
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
    granularities = {
      1.minute => '1min',
      5.minutes => '5min',
      15.minutes => '15min',
      30.minutes => '30min',
      1.hour => '1h',
      4.hours => '4h',
      1.day => '1day',
      3.days => '1day',
      1.week => '1Wutc',
      1.month => '1Mutc'
    }
    granularity = granularities[timeframe]

    limit = 1000
    candles = []
    loop do
      result = client.get_candles(
        symbol: ticker.ticker,
        granularity: granularity,
        start_time: (start_at.to_i * 1000).to_s,
        end_time: (Time.now.utc.to_i * 1000).to_s,
        limit: limit
      )
      return result if result.failure?

      data = result.data['data']
      break if data.blank?

      data.each do |candle|
        candles << [
          Time.at(candle[0].to_i / 1000).utc,
          candle[1].to_d, # open
          candle[2].to_d, # high
          candle[3].to_d, # low
          candle[4].to_d, # close
          candle[5].to_d  # volume
        ]
      end
      break if data.size < limit

      start_at = candles.last[0] + 1.second
    end

    candles = build_candles_from_candles(candles: candles, timeframe: timeframe) if timeframe.in?([3.days])

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

    data = result.data['data']
    return Result::Failure.new("Failed to get #{name} order (order_id: #{order_id})") if data.blank?

    order_data = data.is_a?(Array) ? data.first : data
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
    temp_client = Clients::Bitget.new(
      api_key: api_key.key,
      api_secret: api_key.secret,
      passphrase: api_key.passphrase
    )
    result = temp_client.get_assets

    if result.success?
      Result::Success.new(result.data['code'] == '00000')
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
      coins_result = Clients::Bitget.new.get_coins
      return coins_result if coins_result.failure?

      coin_data = Array(coins_result.data['data']).find { |c| c['coin'] == symbol }
      return Result::Failure.new("No coin data found for #{symbol} on Bitget") if coin_data.blank?

      chains = coin_data['chains'] || []
      chain = chains.find { |c| c['isDefault'] == 'true' } || chains.first
      return Result::Failure.new("No chain found for #{symbol} on Bitget") if chain.blank?

      chain_name = chain['chain']
    end

    result = client.withdraw(coin: symbol, address: address, size: amount.to_d.to_s('F'),
                             chain: chain_name, tag: address_tag)
    return result if result.failure?

    withdrawal_id = result.data.dig('data', 'orderId')
    Result::Success.new({ withdrawal_id: withdrawal_id })
  end

  def fetch_withdrawal_fees!
    result = Clients::Bitget.new.get_coins
    return result if result.failure?

    fees = {}
    chain_data = {}
    Array(result.data['data']).each do |coin|
      symbol = coin['coin']
      coin_chains = coin['chains'] || []
      chain = coin_chains.find { |c| c['isDefault'] == 'true' } || coin_chains.first
      next unless chain

      fees[symbol] = chain['withdrawFee']
      chain_data[symbol] = coin_chains.map do |c|
        { 'name' => c['chain'], 'fee' => c['withdrawFee'], 'is_default' => c['isDefault'] == 'true' }
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
      result = client.get_orderbook(symbol: ticker.ticker, limit: 1)
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
      symbol: ticker.ticker,
      side: side.to_s,
      order_type: 'market',
      force: 'gtc',
      size: amount_type == :base ? amount.to_d.to_s('F') : nil,
      quote_size: amount_type == :quote ? amount.to_d.to_s('F') : nil
    }
    result = client.place_order(**order_settings)
    return result if result.failure?

    data = result.data['data']
    return Result::Failure.new(result.data['msg'] || "Failed to set #{name} market order") if result.data['code'] != '00000'

    order_id = Utilities::Hash.dig_or_raise(data, 'orderId')

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
      symbol: ticker.ticker,
      side: side.to_s,
      order_type: 'limit',
      force: 'gtc',
      price: price.to_d.to_s('F'),
      size: amount.to_d.to_s('F')
    }
    result = client.place_order(**order_settings)
    return result if result.failure?

    data = result.data['data']
    return Result::Failure.new(result.data['msg'] || "Failed to set #{name} limit order") if result.data['code'] != '00000'

    order_id = Utilities::Hash.dig_or_raise(data, 'orderId')

    Result::Success.new({ order_id: order_id })
  end

  def parse_order_data(order_id, order_data)
    symbol = Utilities::Hash.dig_or_raise(order_data, 'symbol')
    ticker = tickers.find_by(ticker: symbol)
    order_type = parse_order_type(Utilities::Hash.dig_or_raise(order_data, 'orderType'))
    price = order_data['priceAvg']&.to_d || order_data['price']&.to_d || 0
    price = nil if price.zero?
    amount = order_data['size']&.to_d
    quote_amount = order_data['quoteSize']&.to_d
    side = Utilities::Hash.dig_or_raise(order_data, 'side').downcase.to_sym
    amount_exec = order_data['baseVolume']&.to_d || 0
    quote_amount_exec = order_data['quoteVolume']&.to_d || 0
    status = parse_order_status(Utilities::Hash.dig_or_raise(order_data, 'status'))

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
    case order_type.downcase
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
    when 'init', 'new'
      :unknown
    when 'partial_fill', 'live'
      :open
    when 'full_fill'
      :closed
    when 'cancelled'
      :cancelled
    else
      raise "Unknown #{name} order status: #{status}"
    end
  end
end
