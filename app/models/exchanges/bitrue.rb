class Exchanges::Bitrue < Exchange
  COINGECKO_ID = 'bitrue'.freeze # https://docs.coingecko.com/reference/exchanges-list
  ERRORS = {
    insufficient_funds: ['Insufficient balance.', 'Account has insufficient balance'],
    invalid_key: ['Invalid Api-Key ID.', 'Signature for this request is not valid.']
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
    @client = Clients::Bitrue.new(
      api_key: api_key&.key,
      api_secret: api_key&.secret
    )
  end

  def get_tickers_info(force: false)
    cache_key = "exchange_#{id}_tickers_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour, force: force) do
      result = client.exchange_information
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error) : result
      end

      result.data['symbols'].filter_map do |product|
        ticker = Utilities::Hash.dig_or_raise(product, 'symbol')
        status = Utilities::Hash.dig_or_raise(product, 'status')

        filters = product['filters'] || []
        price_filter = filters.find { |filter| filter['filterType'] == 'PRICE_FILTER' }
        lot_size_filter = filters.find { |filter| filter['filterType'] == 'LOT_SIZE' }

        {
          ticker: ticker,
          base: product['baseAsset']&.upcase,
          quote: product['quoteAsset']&.upcase,
          minimum_base_size: lot_size_filter&.dig('minQty').to_d,
          minimum_quote_size: lot_size_filter&.dig('minVal').to_d,
          maximum_base_size: lot_size_filter&.dig('maxQty').to_d,
          maximum_quote_size: 0.to_d,
          base_decimals: if lot_size_filter
                           Utilities::Number.decimals(lot_size_filter['stepSize'])
                         else
                           Utilities::Hash.dig_or_raise(product, 'baseAssetPrecision')
                         end,
          quote_decimals: Utilities::Hash.dig_or_raise(product, 'quotePrecision'),
          price_decimals: if price_filter
                            Utilities::Number.decimals(price_filter['tickSize'])
                          else
                            Utilities::Hash.dig_or_raise(product, 'quotePrecision')
                          end,
          available: status == 'TRADING'
        }
      end
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
    result = client.account_information
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
    }
    interval = intervals[timeframe]

    limit = 500
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

      items = result.data.is_a?(Array) ? result.data : []
      items.each do |candle|
        candles << [
          Time.at(candle[0] / 1000).utc,
          candle[1].to_d,
          candle[2].to_d,
          candle[3].to_d,
          candle[4].to_d,
          candle[5].to_d
        ]
      end
      break if items.empty? || items.size < limit

      start_at = candles.last[0] + 1.second
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
    symbol, ext_order_id = order_id.split('-')
    result = client.query_order(symbol: symbol, order_id: ext_order_id)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

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
    symbol, ext_order_id = order_id.split('-')
    result = client.cancel_order(symbol: symbol, order_id: ext_order_id)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

    Result::Success.new(order_id)
  end

  def get_api_key_validity(api_key:)
    result = Clients::Bitrue.new(
      api_key: api_key.key,
      api_secret: api_key.secret
    ).account_information

    if result.success?
      Result::Success.new(true)
    elsif result.data.is_a?(Hash) && result.data[:status] == 401
      Result::Success.new(false)
    else
      error = parse_error_message(result)
      if error.present? && ERRORS[:invalid_key].any? { |msg| error.include?(msg) }
        Result::Success.new(false)
      else
        result
      end
    end
  end

  def minimum_amount_logic(order_type:, **)
    if order_type == :market_order
      :base_and_quote
    else
      :base_and_quote_in_base
    end
  end

  def withdraw(asset:, amount:, address:, network: nil, address_tag: nil)
    symbol = symbol_from_asset(asset)
    return Result::Failure.new("Unknown symbol for asset #{asset.symbol}") if symbol.blank?

    # Use provided network or determine the default
    network_name = network
    if network_name.blank?
      coins_result = client.get_all_coins_information
      return coins_result if coins_result.failure?

      coin_data = Array(coins_result.data).find { |c| c['coin'] == symbol }
      return Result::Failure.new("No coin data found for #{symbol} on Bitrue") if coin_data.blank?

      networks = coin_data['networkList'] || []
      default_network = networks.find { |n| n['isDefault'] == true } || networks.first
      network_name = default_network&.dig('network')
    end

    result = client.withdraw(coin: symbol, address: address, amount: amount.to_d.to_s('F'),
                             network: network_name, address_tag: address_tag)
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

    result = Clients::Bitrue.new(
      api_key: api_key.key,
      api_secret: api_key.secret
    ).get_all_coins_information
    return result if result.failure?

    fees = {}
    chains = {}
    Array(result.data).each do |coin|
      symbol = coin['coin']
      networks = coin['networkList'] || []
      default_net = networks.find { |n| n['isDefault'] == true } || networks.first
      next unless default_net

      fees[symbol] = default_net['withdrawFee']
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

  # @param amount [Float] must be a positive number
  # @param amount_type [Symbol] :base or :quote
  # @param side [Symbol] must be either :buy or :sell
  def set_market_order(ticker:, amount:, amount_type:, side:)
    amount = ticker.adjusted_amount(amount: amount, amount_type: amount_type)

    order_settings = {
      symbol: ticker.ticker,
      side: side.to_s.upcase,
      type: 'MARKET',
      quote_order_qty: amount_type == :quote ? amount.to_d.to_s('F') : nil,
      quantity: amount_type == :base ? amount.to_d.to_s('F') : nil
    }
    result = client.new_order(**order_settings)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error) : result
    end

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
    quote_amount = order_data['origQuoteOrderQty']&.to_d
    quote_amount = nil if quote_amount.nil? || quote_amount.zero?
    side = Utilities::Hash.dig_or_raise(order_data, 'side').downcase.to_sym
    amount_exec = Utilities::Hash.dig_or_raise(order_data, 'executedQty').to_d
    quote_amount_exec = Utilities::Hash.dig_or_raise(order_data, 'cummulativeQuoteQty').to_d
    quote_amount_exec = nil if quote_amount_exec.negative?
    status = parse_order_status(Utilities::Hash.dig_or_raise(order_data, 'status'))
    price = Utilities::Hash.dig_or_raise(order_data, 'price').to_d
    if price.zero? &&
       quote_amount_exec.present? &&
       quote_amount_exec.positive? &&
       amount_exec.positive?
      price = quote_amount_exec / amount_exec
      price = ticker.adjusted_price(price: price, method: :round) if ticker.present?
    end
    price = nil if price.zero?

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
    case status
    when 'NEW', 'PARTIALLY_FILLED'
      :open
    when 'FILLED'
      :closed
    when 'CANCELED', 'EXPIRED'
      :cancelled
    when 'REJECTED'
      :failed
    else
      raise "Unknown #{name} order status: #{status}"
    end
  end
end
