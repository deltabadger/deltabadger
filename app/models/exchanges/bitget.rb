class Exchanges::Bitget < Exchange
  COINGECKO_ID = 'bitget'.freeze # https://docs.coingecko.com/reference/exchanges-list
  ERRORS = {
    insufficient_funds: ['Insufficient balance', 'Insufficient account balance'],
    invalid_key: ['Invalid Api Key', 'Invalid ACCESS_KEY', 'Invalid signature', 'Apikey does not exist']
  }.freeze

  # API-key-validity probe (fake cancel_order) classification by Bitget response code. Kept out of
  # ERRORS (which is message-oriented — callers iterate its messages) because these match on code:
  # order-not-found's msg is localized (订单不存在), so the code is the only stable signal.
  ORDER_NOT_FOUND_CODES = %w[43001 43025].freeze     # probe got past the permission gate ⇒ key can trade
  NO_TRADE_PERMISSION_CODES = %w[40014].freeze       # "Incorrect permissions, need spot order write permissions"
  private_constant :ORDER_NOT_FOUND_CODES, :NO_TRADE_PERMISSION_CODES

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

  def supports_withdrawal?
    false
  end

  def set_client(api_key: nil)
    @api_key = api_key
    @client = Honeymaker.client('bitget',
                                api_key: api_key&.key,
                                api_secret: api_key&.secret,
                                passphrase: api_key&.passphrase,
                                proxy: ENV['PROXY_BITGET'])
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

  def get_tickers_prices(force: false, symbols: nil)
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
    result = client.get_account_assets
    return result if result.failure?

    data = result.data['data']
    return Result::Failure.new("Failed to get #{name} balances") if data.nil?

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.to_h do |asset_id|
      [asset_id, { free: 0, locked: 0 }]
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
    _, ext_order_id = parse_order_id(order_id)
    result = client.get_order(order_id: ext_order_id)
    return result if result.failure?

    normalized_order_data = parse_order_data(order_id, result.data[:raw])

    Result::Success.new(normalized_order_data)
  end

  def get_orders(order_ids:)
    orders = {}
    order_ids.each do |order_id|
      result = get_order(order_id: order_id)
      return result if result.failure?

      orders[order_id] = result.data
    end

    Result::Success.new(orders: orders, missing: [])
  end

  def cancel_order(order_id:)
    symbol, ext_order_id = parse_order_id(order_id)
    if symbol.nil?
      # Legacy order without symbol prefix — look it up
      order_result = client.get_order(order_id: ext_order_id)
      return order_result if order_result.failure?

      symbol = order_result.data[:raw]['symbol']
    end
    result = client.cancel_order(symbol: symbol, order_id: ext_order_id)
    return result if result.failure?

    Result::Success.new(order_id)
  end

  def get_api_key_validity(api_key:)
    temp_client = Honeymaker.client('bitget',
                                    api_key: api_key.key,
                                    api_secret: api_key.secret,
                                    passphrase: api_key.passphrase,
                                    proxy: ENV['PROXY_BITGET'])

    result = if api_key.withdrawal?
               temp_client.get_account_assets
             else
               temp_client.cancel_order(symbol: 'BTCUSDT', order_id: '0')
             end

    classify_api_key_validity(result, api_key)
  end

  def minimum_amount_logic(order_type:, **)
    # Limit orders are base-denominated on Bitget (size + price, no quoteSize), so they must
    # be sized in base — never :quote. Market orders keep the cheaper-minimum choice
    # (set_market_order branches size/quote_size).
    order_type == :limit_order ? :base_and_quote_in_base : :base_or_quote
  end

  def withdraw(asset:, amount:, address:, network: nil, address_tag: nil)
    symbol = symbol_from_asset(asset)
    return Result::Failure.new("Unknown symbol for asset #{asset.symbol}") if symbol.blank?

    # Use provided network or determine the default chain
    chain_name = network
    if chain_name.blank?
      coins_result = Honeymaker.client('bitget', proxy: ENV['PROXY_BITGET']).get_coins
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
    result = Honeymaker.client('bitget', proxy: ENV['PROXY_BITGET']).get_coins
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

  def get_ledger(api_key:, start_time: nil)
    hm_client = Honeymaker.client('bitget', api_key: api_key.key, api_secret: api_key.secret,
                                            passphrase: api_key.passphrase, proxy: ENV['PROXY_BITGET'])
    start_ms = start_time ? (start_time.to_f * 1000).to_i.to_s : nil
    entries = []

    # Fills
    result = hm_client.get_fills(start_time: start_ms)
    unless result.failure?
      (result.data['data'] || []).each do |fill|
        symbol = fill['symbol']
        base = symbol_pair_base(symbol)
        quote = symbol_pair_quote(symbol)
        is_buyer = fill['side']&.downcase == 'buy'
        entries << { entry_type: is_buyer ? :buy : :sell,
                     base_currency: base, base_amount: fill['size'].to_d,
                     quote_currency: quote, quote_amount: (fill['size'].to_d * fill['priceAvg'].to_d),
                     fee_currency: fill['feeDetail']&.dig('feeCoin') || quote,
                     fee_amount: fill['feeDetail']&.dig('totalFee')&.to_d&.abs || fill['fee'].to_d.abs,
                     tx_id: fill['tradeId'], group_id: nil, description: nil,
                     transacted_at: Time.at(fill['cTime'].to_i / 1000.0).utc, raw_data: fill }
      end
    end

    # Deposits
    result = hm_client.deposit_list(start_time: start_ms)
    unless result.failure?
      (result.data['data'] || []).each do |dep|
        next unless dep['status'] == 'success'

        entries << { entry_type: :deposit, base_currency: dep['coin'], base_amount: dep['size'].to_d,
                     quote_currency: nil, quote_amount: nil, fee_currency: nil, fee_amount: nil,
                     tx_id: dep['txId'], group_id: nil, description: nil,
                     transacted_at: Time.at(dep['cTime'].to_i / 1000.0).utc, raw_data: dep }
      end
    end

    # Withdrawals
    result = hm_client.withdrawal_list(start_time: start_ms)
    unless result.failure?
      (result.data['data'] || []).each do |wd|
        next unless wd['status'] == 'success'

        entries << { entry_type: :withdrawal, base_currency: wd['coin'], base_amount: wd['size'].to_d,
                     quote_currency: nil, quote_amount: nil,
                     fee_currency: wd['coin'], fee_amount: wd['fee'].to_d,
                     tx_id: wd['txId'], group_id: nil, description: nil,
                     transacted_at: Time.at(wd['cTime'].to_i / 1000.0).utc, raw_data: wd }
      end
    end

    Result::Success.new(entries)
  end

  private

  # Classifies the api-key-validity probe by Bitget's response code. The code arrives either in a
  # Success envelope (HTTP 200) or — for v2 business errors, which come back as HTTP 4xx — inside a
  # Failure carrying the raw JSON body string (honeymaker's with_rescue). Bitget is the lone
  # cancel-probe exchange that previously read only the Success branch, so HTTP-400 order-not-found
  # (the "key can trade" signal) was wrongly rejected. Mirrors Bitvavo/BingX failure-branch handling.
  def classify_api_key_validity(result, api_key)
    code, msg = bitget_envelope(result)

    if code == '00000'
      Result::Success.new(true)
    elsif ORDER_NOT_FOUND_CODES.include?(code)
      # Probe reached the order layer ⇒ the key has trade permission. Withdrawal keys still need a
      # withdrawal-permission check (done at withdrawal time), which this probe does not prove.
      api_key.withdrawal? ? Result::Success.new(false) : Result::Success.new(true)
    elsif NO_TRADE_PERMISSION_CODES.include?(code) ||
          (msg.present? && ERRORS[:invalid_key].any? { |m| msg.include?(m) }) ||
          (result.data.is_a?(Hash) && result.data[:status] == 401)
      Result::Success.new(false)
    elsif result.success?
      # Recognized envelope, unrecognized non-zero code — preserve prior lenient behavior.
      api_key.withdrawal? ? Result::Success.new(false) : Result::Success.new(true)
    else
      # Genuine transport/unknown failure → surfaces as pending_validation (retryable).
      result
    end
  end

  # Returns [code, msg] from a Bitget probe result, whether it is a Success envelope (HTTP 200) or a
  # Failure carrying the raw JSON body string (HTTP 4xx via honeymaker's with_rescue). Guards on the
  # parsed value's type, not just exceptions: JSON.parse("null")/'"text"' return non-Hash without
  # raising, so a bare rescue would not be enough.
  def bitget_envelope(result)
    return [result.data['code'], result.data['msg']] if result.success? && result.data.is_a?(Hash)

    body = result.errors.first
    parsed = begin
      JSON.parse(body) if body.is_a?(String)
    rescue JSON::ParserError
      nil
    end
    parsed.is_a?(Hash) ? [parsed['code'], parsed['msg']] : [nil, body]
  end

  def client
    @client ||= set_client
  end

  def symbol_pair_base(symbol)
    tickers.find_by(ticker: symbol)&.base || symbol
  end

  def symbol_pair_quote(symbol)
    tickers.find_by(ticker: symbol)&.quote || symbol
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
      force: 'ioc',
      size: amount_type == :base ? amount.to_d.to_s('F') : nil,
      quote_size: amount_type == :quote ? amount.to_d.to_s('F') : nil
    }
    result = client.place_order(**order_settings)
    return result if result.failure?

    Result::Success.new({ order_id: result.data[:order_id] })
  end

  # @param amount [Float] must be a positive number
  # @param amount_type [Symbol] :base or :quote
  # @param side [Symbol] must be either :buy or :sell
  # @param price [Float] must be a positive number
  def set_limit_order(ticker:, amount:, amount_type:, side:, price:)
    price = ticker.adjusted_price(price: price)
    return Result::Failure.new("Invalid limit price for #{ticker.ticker}") unless price.to_d.positive?

    # Bitget limit orders are base-denominated (size + price; no quoteSize). Convert a :quote
    # amount to base at the adjusted limit price so the order reserves the intended quote
    # rather than shipping the quote figure as a base quantity (-> 43012 Insufficient balance).
    base_amount = amount_type == :quote ? amount.to_d / price.to_d : amount
    base_amount = ticker.adjusted_amount(amount: base_amount, amount_type: :base)

    order_settings = {
      symbol: ticker.ticker,
      side: side.to_s,
      order_type: 'limit',
      force: 'gtc',
      price: price.to_d.to_s('F'),
      size: base_amount.to_d.to_s('F')
    }
    result = client.place_order(**order_settings)
    return result if result.failure?

    Result::Success.new({ order_id: result.data[:order_id] })
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

  def parse_order_id(order_id)
    if order_id.include?('-')
      order_id.split('-', 2)
    else
      [nil, order_id]
    end
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
    when 'partially_filled', 'live'
      :open
    when 'filled'
      :closed
    when 'cancelled'
      :cancelled
    else
      raise "Unknown #{name} order status: #{status}"
    end
  end
end
