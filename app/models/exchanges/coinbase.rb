class Exchanges::Coinbase < Exchange
  COINGECKO_ID = 'gdax'.freeze # https://docs.coingecko.com/reference/exchanges-list
  ASSET_BLACKLIST = [
    'RENDER', # has the same external_id as RNDR. Remove it when Coinbase delists RENDER pairs
    'ZETACHAIN', # has the same external_id as ZETA. Remove it when Coinbase delists ZETACHAIN pairs
    'WAXL' # has the same external_id as AXL. Remove it when Coinbase delists WAXL pairs
  ].freeze
  ERRORS = {
    insufficient_funds: ['Insufficient balance in source account']
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
    @proxy_ip ||= Clients::Coinbase::PROXY.split('://').last.split(':').first if Clients::Coinbase::PROXY.present?
  end

  def set_client(api_key: nil)
    @api_key = api_key
    @client = Clients::Coinbase.new(
      api_key: api_key&.key,
      api_secret: api_key&.secret
    )
  end

  def get_tickers_info(force: false)
    cache_key = "exchange_#{id}_tickers_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour, force: force) do
      result = client.list_products
      return Result::Failure.new("Failed to get #{name} products") if result.failure?

      result.data['products'].map do |product|
        ticker = Utilities::Hash.dig_or_raise(product, 'product_id')
        base, quote = ticker.split('-')
        next if base.in?(ASSET_BLACKLIST)

        base_increment = Utilities::Hash.dig_or_raise(product, 'base_increment')
        quote_increment = Utilities::Hash.dig_or_raise(product, 'quote_increment')
        price_increment = Utilities::Hash.dig_or_raise(product, 'price_increment')
        {
          ticker: ticker,
          base: base,
          quote: quote,
          minimum_base_size: Utilities::Hash.dig_or_raise(product, 'base_min_size').to_d,
          minimum_quote_size: Utilities::Hash.dig_or_raise(product, 'quote_min_size').to_d,
          maximum_base_size: Utilities::Hash.dig_or_raise(product, 'base_max_size').to_d,
          maximum_quote_size: Utilities::Hash.dig_or_raise(product, 'quote_max_size').to_d,
          base_decimals: Utilities::Number.decimals(base_increment),
          quote_decimals: Utilities::Number.decimals(quote_increment),
          price_decimals: Utilities::Number.decimals(price_increment)
        }
      end.compact
    end

    Result::Success.new(tickers_info)
  end

  def get_tickers_prices(force: false)
    cache_key = "exchange_#{id}_prices"
    tickers_prices = Rails.cache.fetch(cache_key, expires_in: 1.minute, force: force) do
      result = client.list_products
      return Result::Failure.new("Failed to get #{name} products") if result.failure?

      result.data['products'].each_with_object({}) do |product, prices_hash|
        ticker = Utilities::Hash.dig_or_raise(product, 'product_id')
        price = Utilities::Hash.dig_or_raise(product, 'price').to_d
        prices_hash[ticker] = price
      end
    end

    Result::Success.new(tickers_prices)
  end

  def get_balances(asset_ids: nil)
    result = get_portfolio_uuid
    return result if result.failure?

    portfolio_uuid = result.data
    result = client.get_portfolio_breakdown(portfolio_uuid: portfolio_uuid)
    return result if result.failure?

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.each_with_object({}) do |asset_id, balances_hash|
      balances_hash[asset_id] = { free: 0, locked: 0 }
    end
    breakdown = Utilities::Hash.dig_or_raise(result.data, 'breakdown', 'spot_positions')
    breakdown.each do |position|
      asset = asset_from_symbol(position['asset'])
      next unless asset.present?
      next unless asset_ids.include?(asset.id)

      free = Utilities::Hash.dig_or_raise(position, 'available_to_trade_crypto').to_d
      locked = Utilities::Hash.dig_or_raise(position, 'total_balance_crypto').to_d - free

      balances[asset.id] = { free: free, locked: locked }
    end

    Result::Success.new(balances)
  end

  def get_last_price(ticker:, force: false)
    cache_key = "exchange_#{id}_last_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      result = client.get_product(product_id: ticker.ticker)
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
    granularities = {
      1.minute => 'ONE_MINUTE',
      5.minutes => 'FIVE_MINUTE',
      15.minutes => 'FIFTEEN_MINUTE',
      30.minutes => 'THIRTY_MINUTE',
      1.hour => 'ONE_HOUR',
      4.hours => 'TWO_HOUR',
      1.day => 'ONE_DAY',
      3.days => 'ONE_DAY',
      1.week => 'ONE_DAY',
      1.month => 'ONE_DAY'
      # 2.hours => 'TWO_HOUR',
      # 6.hours => 'SIX_HOUR',
    }
    granularity = granularities[timeframe]

    duration = {
      'ONE_MINUTE' => 1.minute,
      'FIVE_MINUTE' => 5.minutes,
      'FIFTEEN_MINUTE' => 15.minutes,
      'THIRTY_MINUTE' => 30.minutes,
      'ONE_HOUR' => 1.hour,
      'TWO_HOUR' => 2.hours,
      'ONE_DAY' => 1.day
    }

    candles = []
    loop do
      now = Time.now.utc
      end_time = [start_at + 350 * duration[granularity], now].min
      result = client.get_public_product_candles(
        product_id: ticker.ticker,
        start_time: start_at.to_i,
        end_time: end_time.to_i,
        granularity: granularity
      )
      return result if result.failure?

      raw_candles_list = result.data['candles'].sort_by { |candle| candle['start'] }
      raw_candles_list.each do |candle|
        candles << [
          Time.at(candle['start'].to_i).utc,
          candle['open'].to_d,
          candle['high'].to_d,
          candle['low'].to_d,
          candle['close'].to_d,
          candle['volume'].to_d
        ]
      end
      break if end_time == now

      start_at = candles.empty? ? end_time : candles.last[0] + 1.second
    end

    candles = build_candles_from_candles(candles: candles, timeframe: timeframe) if timeframe.in?([4.hours,
                                                                                                   3.days,
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
    result = client.get_order(order_id: order_id)
    return result if result.failure?

    order_data = Utilities::Hash.dig_or_raise(result.data, 'order')
    normalized_order_data = parse_order_data(order_id, order_data)

    Result::Success.new(normalized_order_data)
  end

  def get_orders(order_ids:)
    orders = {}
    order_ids.each_slice(50) do |order_ids_slice|
      result = client.list_orders(order_ids: order_ids_slice)
      return result if result.failure?

      order_datas = Utilities::Hash.dig_or_raise(result.data, 'orders')
      order_datas.each do |order_data|
        order_id = Utilities::Hash.dig_or_raise(order_data, 'order_id')
        orders[order_id] = parse_order_data(order_id, order_data)
      end
    end

    Result::Success.new(orders)
  end

  def cancel_order(order_id:)
    result = client.cancel_orders(order_ids: [order_id])
    return result if result.failure?

    results = Utilities::Hash.dig_or_raise(result.data, 'results')
    success = Utilities::Hash.dig_or_raise(results[0], 'success')
    error = Utilities::Hash.dig_or_raise(results[0], 'failure_reason')
    return Result::Failure.new(error) unless success

    Result::Success.new(order_id)
  end

  def get_api_key_validity(api_key:) # rubocop:disable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity
    result = Clients::Coinbase.new(
      api_key: api_key.key,
      api_secret: api_key.secret
    ).get_api_key_permissions

    if result.success?
      valid = if api_key.trading?
                result.data['can_view'] == true &&
                  result.data['can_trade'] == true &&
                  result.data['can_transfer'] == false
              elsif api_key.withdrawal?
                result.data['can_view'] == true &&
                  result.data['can_trade'] == false &&
                  result.data['can_transfer'] == true
              else
                raise StandardError, 'Invalid API key type'
              end
      Result::Success.new(valid)
    elsif result.data[:status] == 401 # unauthorized (due to invalid key)
      Result::Success.new(false)
    else
      result
    end
  end

  def minimum_amount_logic(**)
    :base_or_quote
  end

  private

  def client
    @client ||= set_client
  end

  def asset_from_symbol(symbol)
    @asset_from_symbol ||= tickers.includes(:base_asset, :quote_asset).each_with_object({}) do |t, map|
      map[t.base] ||= t.base_asset
      map[t.quote] ||= t.quote_asset
    end
    @asset_from_symbol[symbol]
  end

  def get_portfolio_uuid
    @get_portfolio_uuid ||= begin
      result = client.get_api_key_permissions
      return result if result.failure?

      Result::Success.new(result.data['portfolio_uuid'])
    end
  end

  def get_bid_ask_price(ticker)
    cache_key = "exchange_#{id}_bid_ask_price_#{ticker.id}"
    Rails.cache.fetch(cache_key, expires_in: 1.seconds) do
      result = client.get_public_product_book(product_id: ticker.ticker, limit: 1)
      return result if result.failure?

      Result::Success.new(
        {
          bid: {
            price: result.data['pricebook']['bids'][0]['price'].to_d,
            size: result.data['pricebook']['bids'][0]['size'].to_d
          },
          ask: {
            price: result.data['pricebook']['asks'][0]['price'].to_d,
            size: result.data['pricebook']['asks'][0]['size'].to_d
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
      client_order_id: SecureRandom.uuid,
      product_id: ticker.ticker,
      side: side.to_s.upcase,
      order_configuration: {
        market_market_ioc: {
          quote_size: amount_type == :quote ? amount.to_d.to_s('F') : nil,
          base_size: amount_type == :base ? amount.to_d.to_s('F') : nil
        }.compact
      }
    }
    result = client.create_order(**order_settings)
    return result if result.failure?

    return Result::Failure.new(result.data.dig('error_response', 'message'), data: result.data) if result.data['success'] == false

    data = {
      order_id: Utilities::Hash.dig_or_raise(result.data, 'success_response', 'order_id')
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
      client_order_id: SecureRandom.uuid,
      product_id: ticker.ticker,
      side: side.to_s.upcase,
      order_configuration: {
        limit_limit_gtc: {
          quote_size: amount_type == :quote ? amount.to_d.to_s('F') : nil,
          base_size: amount_type == :base ? amount.to_d.to_s('F') : nil,
          limit_price: price.to_d.to_s('F')
        }.compact
      }
    }
    result = client.create_order(**order_settings)
    return result if result.failure?

    return Result::Failure.new(result.data.dig('error_response', 'message'), data: result.data) if result.data['success'] == false

    data = {
      order_id: Utilities::Hash.dig_or_raise(result.data, 'success_response', 'order_id')
    }

    Result::Success.new(data)
  end

  def parse_order_data(order_id, order_data)
    product_id = Utilities::Hash.dig_or_raise(order_data, 'product_id')
    ticker = tickers.find_by(ticker: product_id)
    order_type = parse_order_type(Utilities::Hash.dig_or_raise(order_data, 'order_type'))
    price = Utilities::Hash.dig_or_raise(order_data, 'average_filled_price').to_d
    case order_type
    when :limit_order
      order_config = Utilities::Hash.dig_or_raise(order_data, 'order_configuration', 'limit_limit_gtc')
      amount = order_config['base_size']&.to_d
      quote_amount = order_config['quote_size']&.to_d
      price = order_config['limit_price'].to_d if price.zero?
    when :market_order
      order_config = Utilities::Hash.dig_or_raise(order_data, 'order_configuration', 'market_market_ioc')
      amount = order_config['base_size']&.to_d
      quote_amount = order_config['quote_size']&.to_d
    end
    price = nil if price.zero?
    side = Utilities::Hash.dig_or_raise(order_data, 'side').downcase.to_sym
    amount_exec = Utilities::Hash.dig_or_raise(order_data, 'filled_size').to_d
    total_value = Utilities::Hash.dig_or_raise(order_data, 'total_value_after_fees').to_d
    outstanding = Utilities::Hash.dig_or_raise(order_data, 'outstanding_hold_amount').to_d
    quote_amount_exec = total_value - outstanding
    errors = [
      order_data.dig('order', 'reject_reason').presence,
      order_data.dig('order', 'reject_message').presence,
      order_data.dig('order', 'cancel_message').presence
    ].compact
    status = parse_order_status(Utilities::Hash.dig_or_raise(order_data, 'status'))

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
    when 'MARKET'
      :market_order
    when 'LIMIT'
      :limit_order
    else
      raise "Unknown #{name} order type: #{order_type}"
    end
  end

  def parse_order_status(status)
    # UNKNOWN_ORDER_STATUS, OPEN, FILLED, CANCELLED, EXPIRED, FAILED, PENDING
    case status
    when 'PENDING', 'UNKNOWN_ORDER_STATUS'
      :unknown
    when 'OPEN'
      :open
    when 'FILLED', 'CANCELLED', 'EXPIRED'
      :closed
    when 'FAILED'
      :failed # Warning! This is not a valid external_status.
    else
      raise "Unknown #{name} order status: #{status}"
    end
  end
end
