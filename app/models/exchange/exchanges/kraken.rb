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

  attr_reader :api_key

  def coingecko_id
    COINGECKO_ID
  end

  def set_client(api_key: nil)
    @api_key = api_key
    @client = KrakenClient.new(
      api_key: api_key&.key,
      api_secret: api_key&.secret
    )
  end

  def get_tickers_info
    tickers_info = Rails.cache.fetch("exchange_#{id}_info", expires_in: 1.hour) do
      result = client.get_tradable_asset_pairs
      return Result::Failure.new("Failed to get #{name} tradable asset pairs") unless result.success?

      result.data['result'].map do |_, info|
        ticker = Utilities::Hash.dig_or_raise(info, 'altname')
        next if TICKER_BLACKLIST.include?(ticker)

        wsname = Utilities::Hash.dig_or_raise(info, 'wsname')
        {
          ticker: ticker,
          base: wsname.split('/')[0],
          quote: wsname.split('/')[1],
          minimum_base_size: Utilities::Hash.dig_or_raise(info, 'ordermin').to_d,
          minimum_quote_size: Utilities::Hash.dig_or_raise(info, 'costmin').to_d,
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

  def get_balances(asset_ids: nil)
    result = client.get_extended_balance
    return result unless result.success?

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

  def get_balance(asset_id:)
    result = get_balances(asset_ids: [asset_id])
    return result unless result.success?

    Result::Success.new(result.data[asset_id])
  end

  def get_last_price(base_asset_id:, quote_asset_id:)
    result = get_ticker_information(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
    return result unless result.success?

    price = result.data[:last_trade_closed][:price]
    raise "Wrong last price for #{base_asset_id}-#{quote_asset_id}: #{price}" if price.zero?

    Result::Success.new(price)
  end

  def get_bid_price(base_asset_id:, quote_asset_id:)
    result = get_ticker_information(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
    return result unless result.success?

    price = result.data[:bid][:price]
    raise "Wrong bid price for #{base_asset_id}-#{quote_asset_id}: #{price}" if price.zero?

    Result::Success.new(price)
  end

  def get_ask_price(base_asset_id:, quote_asset_id:)
    result = get_ticker_information(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
    return result unless result.success?

    price = result.data[:ask][:price]
    raise "Wrong ask price for #{base_asset_id}-#{quote_asset_id}: #{price}" if price.zero?

    Result::Success.new(price)
  end

  # @param amount_type [Symbol] :base or :quote
  def market_buy(base_asset_id:, quote_asset_id:, amount:, amount_type:)
    set_market_order(
      base_asset_id: base_asset_id,
      quote_asset_id: quote_asset_id,
      amount: amount,
      amount_type: amount_type,
      side: 'buy'
    )
  end

  # @param amount_type [Symbol] :base or :quote
  def market_sell(base_asset_id:, quote_asset_id:, amount:, amount_type:)
    set_market_order(
      base_asset_id: base_asset_id,
      quote_asset_id: quote_asset_id,
      amount: amount,
      amount_type: amount_type,
      side: 'sell'
    )
  end

  # @param amount_type [Symbol] :base or :quote
  def limit_buy(base_asset_id:, quote_asset_id:, amount:, amount_type:, price:)
    set_limit_order(
      base_asset_id: base_asset_id,
      quote_asset_id: quote_asset_id,
      amount: amount,
      amount_type: amount_type,
      side: 'buy',
      price: price
    )
  end

  # @param amount_type [Symbol] :base or :quote
  def limit_sell(base_asset_id:, quote_asset_id:, amount:, amount_type:, price:)
    set_limit_order(
      base_asset_id: base_asset_id,
      quote_asset_id: quote_asset_id,
      amount: amount,
      amount_type: amount_type,
      side: 'sell',
      price: price
    )
  end

  def get_order(order_id:)
    result = client.query_orders_info(txid: order_id)
    return result unless result.success?

    order_data = Utilities::Hash.dig_or_raise(result.data, 'result').map { |_, v| v }.first

    pair = Utilities::Hash.dig_or_raise(order_data, 'descr', 'pair')
    ticker = tickers.find_by(ticker: pair)
    base_asset = ticker.base_asset
    quote_asset = ticker.quote_asset

    rate = Utilities::Hash.dig_or_raise(order_data, 'price').to_d
    amount = Utilities::Hash.dig_or_raise(order_data, 'vol_exec').to_d
    quote_amount = Utilities::Hash.dig_or_raise(order_data, 'cost').to_d
    side = Utilities::Hash.dig_or_raise(order_data, 'descr', 'type').downcase.to_sym
    error_messages = [
      order_data['reason'].presence,
      order_data['misc'].presence
    ].compact
    status = parse_order_status(Utilities::Hash.dig_or_raise(order_data, 'status'))

    Result::Success.new({
                          order_id: order_id,
                          base_asset: base_asset,
                          quote_asset: quote_asset,
                          rate: rate,
                          amount: amount,
                          quote_amount: quote_amount,
                          side: side,
                          error_messages: error_messages,
                          status: status,
                          exchange_response: result.data
                        })
  end

  def check_valid_api_key?(api_key:)
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
               raise StandardError, 'Invalid API key'
             end
    return result unless result.success?

    valid = Utilities::Hash.dig_or_raise(result.data, 'error').empty?
    Result::Success.new(valid)
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

  def get_ticker_information(base_asset_id:, quote_asset_id:) # rubocop:disable Metrics/MethodLength
    Rails.cache.fetch("exchange_#{id}_ticker_information_#{base_asset_id}_#{quote_asset_id}", expires_in: 1.second) do # rubocop:disable Metrics/BlockLength
      ticker = tickers.find_by(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
      return Result::Failure.new("No ticker found for #{base_asset_id} and #{quote_asset_id}") unless ticker

      result = client.get_ticker_information(pair: ticker.ticker)
      return result unless result.success?

      asset_ticker_info = Utilities::Hash.dig_or_raise(result.data, 'result').map { |_, v| v }.first
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

  # @param amount: Float must be a positive number
  # @param amount_type [Symbol] :base or :quote
  # @param side: String must be either 'buy' or 'sell'
  def set_market_order(base_asset_id:, quote_asset_id:, amount:, amount_type:, side:)
    ticker = tickers.find_by(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
    return Result::Failure.new("No ticker found for #{base_asset_id} and #{quote_asset_id}") unless ticker

    adjusted_amount = adjusted_amount(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id,
                                      amount: amount, amount_type: amount_type)

    client_order_id = SecureRandom.uuid
    result = client.add_order(
      cl_ord_id: client_order_id,
      ordertype: 'market',
      type: side.downcase,
      volume: adjusted_amount.to_d.to_s('F'),
      pair: ticker.ticker,
      oflags: amount_type == :quote ? ['viqc'] : []
    )
    return result unless result.success?

    return Result::Failure.new(result.data['error'].to_sentence, data: result.data) if result.data['error'].any?

    data = {
      order_id: Utilities::Hash.dig_or_raise(result.data, 'result', 'txid').first
    }

    Result::Success.new(data)
  end

  # @param amount: Float must be a positive number
  # @param amount_type [Symbol] :base or :quote
  # @param side: String must be either 'buy' or 'sell'
  # @param price: Float must be a positive number
  def set_limit_order(base_asset_id:, quote_asset_id:, amount:, amount_type:, side:, price:)
    ticker = tickers.find_by(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
    return Result::Failure.new("No ticker found for #{base_asset_id} and #{quote_asset_id}") unless ticker

    adjusted_amount = adjusted_amount(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id,
                                      amount: amount, amount_type: amount_type)
    adjusted_price = adjusted_price(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id,
                                    price: price)

    client_order_id = SecureRandom.uuid
    result = client.add_order(
      cl_ord_id: client_order_id,
      ordertype: 'limit',
      type: side.downcase,
      volume: adjusted_amount.to_d.to_s('F'),
      pair: ticker.ticker,
      price: adjusted_price.to_d.to_s('F'),
      oflags: amount_type == :quote ? ['viqc'] : []
    )
    return result unless result.success?

    return Result::Failure.new(result.data['error'].to_sentence, data: result.data) if result.data['error'].any?

    data = {
      order_id: Utilities::Hash.dig_or_raise(result.data, 'result', 'txid').first
    }

    Result::Success.new(data)
  end

  def parse_order_status(status)
    # pending, open, closed, canceled, expired
    case status
    when 'closed'
      :success
    when 'canceled', 'expired'
      :failure
    else
      :unknown
    end
  end
end
