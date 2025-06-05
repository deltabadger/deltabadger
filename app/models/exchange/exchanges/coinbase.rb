module Exchange::Exchanges::Coinbase
  extend ActiveSupport::Concern

  COINGECKO_ID = 'gdax'.freeze # https://docs.coingecko.com/reference/exchanges-list
  TICKER_BLACKLIST = [
    'RENDER-USD', # same as RNDR-USD. Remove it when Coinbase delists RENDER-USD
    'RENDER-USDC',
    'ZETACHAIN-USD', # same as ZETA-USD. Remove it when Coinbase delists ZETACHAIN-USD
    'ZETACHAIN-USDC',
    'WAXL-USD', # same as AXL-USD. Remove it when Coinbase delists WAXL-USD
    'WAXL-USDC'
  ].freeze

  attr_reader :api_key

  def coingecko_id
    COINGECKO_ID
  end

  def set_client(api_key: nil)
    @api_key = api_key
    @client = CoinbaseClient.new(
      api_key: api_key&.key,
      api_secret: api_key&.secret
    )
  end

  def get_tickers_info
    cache_key = "exchange_#{id}_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour) do
      result = client.list_products
      return Result::Failure.new("Failed to get #{name} products") if result.failure?

      result.data['products'].map do |product|
        ticker = Utilities::Hash.dig_or_raise(product, 'product_id')
        next if TICKER_BLACKLIST.include?(ticker)

        base_increment = Utilities::Hash.dig_or_raise(product, 'base_increment')
        quote_increment = Utilities::Hash.dig_or_raise(product, 'quote_increment')
        price_increment = Utilities::Hash.dig_or_raise(product, 'price_increment')
        {
          ticker: ticker,
          base: ticker.split('-')[0],
          quote: ticker.split('-')[1],
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
      asset = asset_from_symbol(symbol: position['asset'])
      next unless asset.present?
      next unless asset_ids.include?(asset.id)

      free = Utilities::Hash.dig_or_raise(position, 'available_to_trade_crypto').to_d
      locked = Utilities::Hash.dig_or_raise(position, 'total_balance_crypto').to_d - free

      balances[asset.id] = { free: free, locked: locked }
    end

    Result::Success.new(balances)
  end

  def get_balance(asset_id:)
    result = get_balances(asset_ids: [asset_id])
    return result if result.failure?

    Result::Success.new(result.data[asset_id])
  end

  def get_last_price(ticker:, force: false)
    cache_key = "exchange_#{id}_last_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      result = get_product(ticker: ticker)
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
      result = get_bid_ask_price(ticker: ticker)
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
      result = get_bid_ask_price(ticker: ticker)
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
      2.hours => 'TWO_HOUR', # unique to coinbase
      6.hours => 'SIX_HOUR', # unique to coinbas
      1.day => 'ONE_DAY'
    }
    granularity = granularities[timeframe]

    candles = []
    loop do
      now = Time.now.utc
      end_time = [start_at + 350 * timeframe, now].min
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

    Result::Success.new(candles)
  end

  # @param amount_type [Symbol] :base or :quote
  def market_buy(ticker:, amount:, amount_type:)
    set_market_order(
      ticker: ticker,
      amount: amount,
      amount_type: amount_type,
      side: 'buy'
    )
  end

  # @param amount_type [Symbol] :base or :quote
  def market_sell(ticker:, amount:, amount_type:)
    set_market_order(
      ticker: ticker,
      amount: amount,
      amount_type: amount_type,
      side: 'sell'
    )
  end

  # @param amount_type [Symbol] :base or :quote
  def limit_buy(ticker:, amount:, amount_type:, price:)
    set_limit_order(
      ticker: ticker,
      amount: amount,
      amount_type: amount_type,
      side: 'buy',
      price: price
    )
  end

  # @param amount_type [Symbol] :base or :quote
  def limit_sell(ticker:, amount:, amount_type:, price:)
    set_limit_order(
      ticker: ticker,
      amount: amount,
      amount_type: amount_type,
      side: 'sell',
      price: price
    )
  end

  def get_order(order_id:)
    result = client.get_order(order_id: order_id)
    return result if result.failure?

    product_id = Utilities::Hash.dig_or_raise(result.data, 'order', 'product_id')
    rate = Utilities::Hash.dig_or_raise(result.data, 'order', 'average_filled_price').to_d
    amount = Utilities::Hash.dig_or_raise(result.data, 'order', 'filled_size').to_d
    quote_amount = Utilities::Hash.dig_or_raise(result.data, 'order', 'total_value_after_fees').to_d
    side = Utilities::Hash.dig_or_raise(result.data, 'order', 'side').downcase.to_sym
    error_messages = [
      result.data.dig('order', 'reject_reason').presence,
      result.data.dig('order', 'reject_message').presence,
      result.data.dig('order', 'cancel_message').presence
    ].compact
    status = parse_order_status(Utilities::Hash.dig_or_raise(result.data, 'order', 'status'))
    ticker = tickers.find_by(ticker: product_id)

    Result::Success.new({
                          order_id: order_id,
                          ticker: ticker,
                          rate: rate,
                          amount: amount,             # amount the account balance went up or down
                          quote_amount: quote_amount, # amount the account balance went up or down
                          side: side,
                          error_messages: error_messages,
                          status: status,
                          exchange_response: result.data
                        })
  end

  def check_valid_api_key?(api_key:)
    result = CoinbaseClient.new(
      api_key: api_key.key,
      api_secret: api_key.secret
    ).get_api_key_permissions
    return result if result.failure?

    valid = if api_key.trading?
              result.data['can_trade'] == true && result.data['can_transfer'] == false
            elsif api_key.withdrawal?
              result.data['can_transfer'] == true
            else
              raise StandardError, 'Invalid API key'
            end

    Result::Success.new(valid)
  end

  def minimum_amount_logic
    :base_or_quote
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
    @asset_from_symbol[symbol]
  end

  def get_portfolio_uuid
    @get_portfolio_uuid ||= begin
      result = client.get_api_key_permissions
      return result if result.failure?

      Result::Success.new(result.data['portfolio_uuid'])
    end
  end

  def get_product(ticker:)
    result = client.get_product(product_id: ticker.ticker)
    return result if result.failure?

    Result::Success.new(result.data)
  end

  def get_bid_ask_price(ticker:)
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

  # @param amount: Float must be a positive number
  # @param amount_type [Symbol] :base or :quote
  # @param side: String must be either 'buy' or 'sell'
  def set_market_order(ticker:, amount:, amount_type:, side:)
    amount = ticker.adjusted_amount(amount: amount, amount_type: amount_type)

    client_order_id = SecureRandom.uuid
    result = client.create_order(
      client_order_id: client_order_id,
      product_id: ticker.ticker,
      side: side.upcase,
      order_configuration: {
        market_market_ioc: {
          quote_size: amount_type == :quote ? amount.to_d.to_s('F') : nil,
          base_size: amount_type == :base ? amount.to_d.to_s('F') : nil
        }.compact
      }
    )
    return result if result.failure?

    return Result::Failure.new(result.data.dig('error_response', 'message'), data: result.data) if result.data['success'] == false

    data = {
      order_id: Utilities::Hash.dig_or_raise(result.data, 'success_response', 'order_id')
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

    client_order_id = SecureRandom.uuid
    result = client.create_order(
      client_order_id: client_order_id,
      product_id: ticker.ticker,
      side: side.upcase,
      order_configuration: {
        limit_limit_gtc: {
          quote_size: amount_type == :quote ? amount.to_d.to_s('F') : nil,
          base_size: amount_type == :base ? amount.to_d.to_s('F') : nil,
          limit_price: price.to_d.to_s('F')
        }.compact
      }
    )
    return result if result.failure?

    return Result::Failure.new(result.data.dig('error_response', 'message'), data: result.data) if result.data['success'] == false

    data = {
      order_id: Utilities::Hash.dig_or_raise(result.data, 'success_response', 'order_id')
    }

    Result::Success.new(data)
  end

  def parse_order_status(status)
    # PENDING, OPEN, FILLED, CANCELLED, EXPIRED, FAILED, UNKNOWN_ORDER_STATUS, QUEUED, CANCEL_QUEUED
    case status
    when 'FILLED'
      :success
    when 'CANCELLED', 'EXPIRED', 'FAILED', 'CANCEL_QUEUED'
      :failure
    else
      :unknown
    end
  end
end
