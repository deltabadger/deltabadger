class Exchanges::Bybit < Exchange
  COINGECKO_ID = 'bybit_spot'.freeze # https://docs.coingecko.com/reference/exchanges-list
  ERRORS = {
    # '170131' removed: the client returns retMsg only (Clients::Bybit#create_order/#get_order)
    # and parse_error_message re-strips the code, so the digits never reach a message — while as
    # a substring they would match any id or epoch that happens to contain them.
    insufficient_funds: ['Insufficient balance'],
    # Messages, not codes — this list is substring-matched against error text by
    # Exchange#invalid_key_error?. It previously held the bare codes 10003/10004, which no message
    # ever contains, so a dead Bybit key was never flagged. The string below is Bybit's actual
    # wording, taken from recorded failures on this venue; it reads like Binance's because Bybit
    # reuses that phrasing. The codes moved to INVALID_KEY_CODES, which is what the retCode branch
    # of get_api_key_validity actually needs.
    # 'API key is invalid' is retCode 10003 as Bybit answers it (https://bybit-exchange.github.io/docs/v5/error);
    # the client keeps only retMsg, so without it a revoked key surfaced as a raw string.
    invalid_key: ['Invalid API-key, IP, or permissions for action.', 'API key is invalid'],
    # retCode 10005: the key is good but lacks a scope, so it must not be condemned.
    permission_denied: ['Permission denied, please check your API key permissions'],
    # retCode 10002, a skewed clock. A read can simply be retried; placement already treats it as safe
    # through Exchange::PLACEMENT_SAFE_TRANSIENT_ERRORS.
    transient: ['Timestamp for this request is outside of the recvWindow']
  }.freeze

  # Matched against the parsed retCode, never against a message — keeping them out of ERRORS stops
  # a bare number being substring-matched into any id or timestamp that happens to contain it.
  # Same split Bitget already uses for its probe codes.
  INVALID_KEY_CODES = %w[10003 10004].freeze
  private_constant :INVALID_KEY_CODES
  # https://bybit-exchange.github.io/docs/v5/enum#orderstatus
  ORDER_STATUS_MAP = {
    'Created' => :unknown,
    'Untriggered' => :unknown,
    'New' => :open,
    'PartiallyFilled' => :open,
    'PartiallyFilledCanceled' => :cancelled,
    'Filled' => :closed,
    'Cancelled' => :cancelled,
    'Expired' => :cancelled,
    'Deactivated' => :cancelled,
    'Rejected' => :failed
  }.freeze

  include Exchange::Dryable # decorators for: get_order, get_orders, cancel_order, get_api_key_validity, set_market_order, set_limit_order

  attr_reader :api_key

  def coingecko_id
    COINGECKO_ID
  end

  def known_errors
    ERRORS
  end

  def supports_withdrawal?
    false
  end

  def set_client(api_key: nil)
    @api_key = api_key
    @client = Honeymaker.client('bybit',
                                api_key: api_key&.key,
                                api_secret: api_key&.secret,
                                proxy: ExchangeProxy.for('bybit'))
  end

  def get_tickers_info(force: false)
    cache_key = "exchange_#{id}_tickers_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour, force: force) do
      result = client.instruments_info(category: 'spot')
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error, data: result.data) : result
      end

      ret_code = result.data['retCode']
      return Result::Failure.new(result.data['retMsg']) if ret_code != 0

      items = Utilities::Hash.dig_or_raise(result.data, 'result', 'list')
      items.map do |product|
        ticker = Utilities::Hash.dig_or_raise(product, 'symbol')
        status = Utilities::Hash.dig_or_raise(product, 'status')

        lot_size_filter = Utilities::Hash.dig_or_raise(product, 'lotSizeFilter')
        price_filter = Utilities::Hash.dig_or_raise(product, 'priceFilter')

        {
          ticker: ticker,
          base: Utilities::Hash.dig_or_raise(product, 'baseCoin'),
          quote: Utilities::Hash.dig_or_raise(product, 'quoteCoin'),
          minimum_base_size: lot_size_filter['minOrderQty'].to_d,
          minimum_quote_size: lot_size_filter['minOrderAmt'].to_d,
          maximum_base_size: lot_size_filter['maxOrderQty'].to_d,
          maximum_quote_size: lot_size_filter['maxOrderAmt'].to_d,
          base_decimals: Utilities::Number.decimals(lot_size_filter['basePrecision']),
          quote_decimals: Utilities::Number.decimals(lot_size_filter['quotePrecision']),
          price_decimals: Utilities::Number.decimals(price_filter['tickSize']),
          available: true,
          trading_enabled: status == 'Trading'
        }
      end.compact
    end

    Result::Success.new(tickers_info)
  end

  def get_tickers_prices(force: false, symbols: nil)
    cache_key = "exchange_#{id}_prices"
    tickers_prices = Rails.cache.fetch(cache_key, expires_in: 1.minute, force: force) do
      result = client.tickers(category: 'spot')
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error, data: result.data) : result
      end

      ret_code = result.data['retCode']
      return Result::Failure.new(result.data['retMsg']) if ret_code != 0

      items = Utilities::Hash.dig_or_raise(result.data, 'result', 'list')
      items.each_with_object({}) do |item, prices_hash|
        ticker = Utilities::Hash.dig_or_raise(item, 'symbol')
        price = Utilities::Hash.dig_or_raise(item, 'lastPrice').to_d
        prices_hash[ticker] = price
      end
    end

    Result::Success.new(tickers_prices)
  end

  def get_balances(asset_ids: nil)
    result = client.wallet_balance(account_type: 'UNIFIED')
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error, data: result.data) : result
    end

    ret_code = result.data['retCode']
    return Result::Failure.new(result.data['retMsg']) if ret_code != 0

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.to_h do |asset_id|
      [asset_id, { free: 0, locked: 0 }]
    end

    accounts = Utilities::Hash.dig_or_raise(result.data, 'result', 'list')
    accounts.each do |account|
      coins = Utilities::Hash.dig_or_raise(account, 'coin')
      coins.each do |coin|
        asset = asset_from_symbol(coin['coin'])
        next unless asset.present?
        next unless asset_ids.include?(asset.id)

        locked = coin['locked'].to_d
        balances[asset.id] = { free: free_balance(coin, locked), locked: locked }
      end
    end

    Result::Success.new(balances)
  end

  def get_last_price(ticker:, force: false)
    cache_key = "exchange_#{id}_last_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      result = client.tickers(category: 'spot', symbol: ticker.ticker)
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error, data: result.data) : result
      end

      ret_code = result.data['retCode']
      return Result::Failure.new(result.data['retMsg']) if ret_code != 0

      items = Utilities::Hash.dig_or_raise(result.data, 'result', 'list')
      price = Utilities::Hash.dig_or_raise(items.first, 'lastPrice').to_d
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
        return error.present? ? Result::Failure.new(error, data: result.data) : result
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
      1.minute => '1',
      5.minutes => '5',
      15.minutes => '15',
      30.minutes => '30',
      1.hour => '60',
      4.hours => '240',
      1.day => 'D',
      3.days => 'D',
      1.week => 'W',
      1.month => 'M'
    }
    interval = intervals[timeframe]

    limit = 1000
    candles = []
    loop do
      result = client.kline(
        category: 'spot',
        symbol: ticker.ticker,
        interval: interval,
        start: start_at.to_i * 1000,
        limit: limit
      )
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error, data: result.data) : result
      end

      ret_code = result.data['retCode']
      return Result::Failure.new(result.data['retMsg']) if ret_code != 0

      items = Utilities::Hash.dig_or_raise(result.data, 'result', 'list')
      # Bybit returns candles in reverse chronological order
      items.reverse_each do |candle|
        candles << [
          Time.at(candle[0].to_i / 1000).utc,
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

  def get_order(order_id:)
    result = find_order(order_id)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error, data: result.data) : result
    end

    normalized_order_data = parse_order_data(order_id, result.data[:raw])

    Result::Success.new(normalized_order_data)
  end

  def get_orders(order_ids:)
    orders = {}
    missing = []
    order_ids.each do |order_id|
      result = get_order(order_id: order_id)
      if result.failure?
        # A gone order goes under :missing for the stale-order sweep; any other failure still fails
        # the batch so the job retries.
        next missing << order_id if result.data.is_a?(Hash) && result.data[:not_found]

        return result
      end

      orders[order_id] = result.data
    end

    Result::Success.new(orders: orders, missing: missing)
  end

  def cancel_order(order_id:)
    # Need to find the symbol for the order
    get_result = find_order(order_id)
    if get_result.failure?
      error = parse_error_message(get_result)
      return error.present? ? Result::Failure.new(error) : get_result
    end

    symbol = get_result.data[:raw]['symbol']
    return Result::Failure.new("Failed to find #{name} order for cancellation") if symbol.blank?

    result = client.cancel_order(category: 'spot', symbol: symbol, order_id: order_id)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error, data: result.data) : result
    end

    ret_code = result.data['retCode']
    return Result::Failure.new(result.data['retMsg']) if ret_code != 0

    Result::Success.new(order_id)
  end

  # Decided by Bybit's documented retCodes (https://bybit-exchange.github.io/docs/v5/error), read from
  # the envelope or an error body. The trading probe cancels an order that does not exist: "order does
  # not exist" means the key got past the permission check. Anything else — a rate limit, a clock
  # error, an unknown code, no answer — leaves the key unverified.
  KEY_CHECK_VALID = %w[170213 110001].freeze
  KEY_CHECK_REJECTED = %w[10003 10004 10005].freeze # invalid key, bad signature, permission denied

  def get_api_key_validity(api_key:)
    temp_client = Honeymaker.client('bybit',
                                    api_key: api_key.key,
                                    api_secret: api_key.secret,
                                    proxy: ExchangeProxy.for('bybit'))

    result = if api_key.withdrawal?
               temp_client.wallet_balance(account_type: 'UNIFIED')
             else
               temp_client.cancel_order(category: 'spot', symbol: 'BTCUSDT', order_id: '0')
             end

    code = response_field(result, 'retCode')&.to_s
    return Result::Success.new(true) if code == '0'
    return Result::Success.new(true) if !api_key.withdrawal? && code.in?(KEY_CHECK_VALID)
    return Result::Success.new(false) if code.in?(KEY_CHECK_REJECTED)

    error = response_field(result, 'retMsg') || parse_error_message(result)
    return Result::Success.new(false) if error.present? && ERRORS[:invalid_key].any? { |msg| error.include?(msg) }

    result.failure? ? result : Result::Failure.new(error.presence || "Bybit answered retCode #{code.inspect}")
  end

  def minimum_amount_logic(order_type:, **)
    # Limit orders are base-denominated on Bybit spot (qty + price), so they must be sized in
    # base — never :quote. Market orders keep the cheaper-minimum choice.
    order_type == :limit_order ? :base_and_quote_in_base : :base_or_quote
  end

  def withdraw(asset:, amount:, address:, network: nil, address_tag: nil)
    symbol = symbol_from_asset(asset)
    return Result::Failure.new("Unknown symbol for asset #{asset.symbol}") if symbol.blank?

    # Use provided network or determine the default chain
    chain_name = network
    if chain_name.blank?
      coin_result = client.get_coin_query_info
      return coin_result if coin_result.failure?

      rows = coin_result.data.dig('result', 'rows') || []
      coin_data = rows.find { |r| r['coin'] == symbol }
      return Result::Failure.new("No coin data found for #{symbol} on Bybit") if coin_data.blank?

      chains = coin_data['chains'] || []
      chain = chains.find { |c| c['chainDefault'] == '1' } || chains.first
      return Result::Failure.new("No chain found for #{symbol} on Bybit") if chain.blank?

      chain_name = chain['chain']
    end

    result = client.withdraw(coin: symbol, chain: chain_name, address: address,
                             amount: amount.to_d.to_s('F'), tag: address_tag)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error, data: result.data) : result
    end

    withdrawal_id = result.data.dig('result', 'id')
    Result::Success.new({ withdrawal_id: withdrawal_id })
  end

  def fetch_withdrawal_fees!
    with_authenticated_fee_client('bybit', 'PROXY_BYBIT') do |hm_client|
      result = hm_client.get_coin_query_info
      return result if result.failure?

      fees = {}
      chain_data = {}
      rows = result.data.dig('result', 'rows') || []
      rows.each do |coin|
        symbol = coin['coin']
        coin_chains = coin['chains'] || []
        chain = coin_chains.find { |c| c['chainDefault'] == '1' } || coin_chains.first
        next unless chain

        fees[symbol] = chain['withdrawFee']
        chain_data[symbol] = coin_chains.map do |c|
          { 'name' => c['chain'], 'fee' => c['withdrawFee'], 'is_default' => c['chainDefault'] == '1' }
        end
      end

      update_exchange_asset_fees!(fees, chains: chain_data)
    end
  end

  # `/v5/execution/list` serves at most 7 days per request, measured from startTime, and reaches two
  # years back (https://bybit-exchange.github.io/docs/v5/order/execution). get_ledger walks 7-day
  # windows from start_time to now itself, so the sync needs no floor and ledger_window stays nil: a
  # floor would drop everything between a quiet account's last sync and a week ago.
  TRADE_WINDOW = 7.days
  # A day inside the two years Bybit keeps, so the oldest window is never refused.
  HISTORY_REACH = 729.days
  # Deposit and withdrawal records want endTime - startTime under 30 days
  # (https://bybit-exchange.github.io/docs/v5/asset/deposit/deposit-record).
  TRANSFER_WINDOW = 29.days

  def get_ledger(api_key:, start_time: nil)
    hm_client = Honeymaker.client('bybit', api_key: api_key.key, api_secret: api_key.secret,
                                           proxy: ExchangeProxy.for('bybit'))
    entries = []

    # Any failed read fails the whole fetch: a partial Success would let the sync move its watermark
    # past the rows it never got, and they would never be fetched again. That includes a rejection
    # inside an HTTP-200 envelope (retCode 10006 "Too many visits"), which the client hands over as
    # a Success.

    from = [start_time, HISTORY_REACH.ago].compact.max
    each_window(from, TRADE_WINDOW) do |window_start, window_end|
      fetch = lambda do |cursor|
        hm_client.execution_list(category: 'spot', start_time: window_start, end_time: window_end, cursor: cursor)
      end
      failed = each_ledger_row(fetch, 'list') do |trade|
        symbol = trade['symbol']
        entries << {
          entry_type: trade['side'] == 'Buy' ? :buy : :sell,
          base_currency: symbol_pair_base(symbol), base_amount: trade['execQty'].to_d,
          quote_currency: symbol_pair_quote(symbol), quote_amount: trade['execValue'].to_d,
          fee_currency: trade['feeCurrency'], fee_amount: trade['execFee'].to_d.abs,
          tx_id: trade['execId'], group_id: nil, description: nil,
          transacted_at: Time.at(trade['execTime'].to_i / 1000.0).utc, raw_data: trade
        }
      end
      return failed if failed
    end

    each_window(from, TRANSFER_WINDOW) do |window_start, window_end|
      fetch = ->(cursor) { hm_client.deposit_records(start_time: window_start, end_time: window_end, cursor: cursor) }
      failed = each_ledger_row(fetch, 'rows', optional: true) do |dep|
        next unless dep['status'].to_i == 3 # success

        entries << { entry_type: :deposit, base_currency: dep['coin'], base_amount: dep['amount'].to_d,
                     quote_currency: nil, quote_amount: nil, fee_currency: nil, fee_amount: nil,
                     tx_id: dep['txID'], group_id: nil, description: nil,
                     transacted_at: Time.at(dep['successAt'].to_i / 1000.0).utc, raw_data: dep }
      end
      return failed if failed

      fetch = ->(cursor) { hm_client.withdraw_records(start_time: window_start, end_time: window_end, cursor: cursor) }
      failed = each_ledger_row(fetch, 'rows', optional: true) do |wd|
        next unless wd['status'] == 'success'

        entries << { entry_type: :withdrawal, base_currency: wd['coin'], base_amount: wd['amount'].to_d,
                     quote_currency: nil, quote_amount: nil,
                     fee_currency: wd['coin'], fee_amount: wd['withdrawFee'].to_d,
                     tx_id: wd['txID'], group_id: nil, description: nil,
                     transacted_at: Time.at(wd['updateTime'].to_i / 1000.0).utc, raw_data: wd }
      end
      return failed if failed
    end

    # Neighbouring windows share their edge, so a row stamped on it comes back twice.
    Result::Success.new(entries.uniq { |entry| [entry[:entry_type], entry[:raw_data]] })
  end

  private

  # Yields [start_ms, end_ms] windows of at most `span`, from `from` to now.
  def each_window(from, span)
    window_start = (from.to_f * 1000).to_i
    now_ms = (Time.now.to_f * 1000).to_i
    while window_start < now_ms
      window_end = [window_start + (span.to_i * 1000), now_ms].min
      yield window_start, window_end
      window_start = window_end
    end
  end

  # retCode 10006, "Too many visits". A first sync walks some hundred weekly windows, and failing part
  # way would throw every page away and start over next time, so a throttled page is waited out and
  # asked for again, as KuCoin's ledger does.
  LEDGER_THROTTLED = 10_006
  LEDGER_THROTTLE_WAIT = 2
  LEDGER_THROTTLE_RETRIES = 5

  # Bybit pages with nextPageCursor. Yields each row under `key`; returns the failure that stopped the
  # read, or nil.
  def each_ledger_row(fetch, key, optional: false, &)
    cursor = nil
    throttled = 0
    loop do
      result = fetch.call(cursor)
      if result.success? && result.data['retCode'].to_i == LEDGER_THROTTLED && throttled < LEDGER_THROTTLE_RETRIES
        throttled += 1
        sleep(LEDGER_THROTTLE_WAIT)
        next
      end
      throttled = 0

      result = checked_ledger_read(result, optional: optional)
      return result if result.failure?

      rows = Array(result.data.dig('result', key))
      rows.each(&)
      cursor = result.data.dig('result', 'nextPageCursor')
      return nil if rows.empty? || cursor.blank?
    end
  end

  # 10005 is "Permission denied". The key setup asks for Spot - Trade only, so a key without the
  # scope for wallet history is the normal case; there the read has nothing to give and counts as
  # empty, as it did before. Spot trades are what the
  # key is for, so a 10005 there fails like any other code.
  LEDGER_NOT_PERMITTED = 10_005

  def checked_ledger_read(result, optional: false)
    return result if result.failure?

    ret_code = result.data['retCode']
    return result if ret_code.to_s == '0'
    return Result::Success.new({}) if optional && ret_code.to_i == LEDGER_NOT_PERMITTED

    Result::Failure.new("#{name} ledger read failed: #{ret_code} #{result.data['retMsg']}")
  end

  # `availableToWithdraw` and `free` are deprecated for unified accounts since 9 Jan 2025 and come
  # back blank, which read as 0. What is free is the wallet balance less what open orders lock and
  # what was borrowed (https://bybit-exchange.github.io/docs/v5/account/wallet-balance). Not yet
  # checked against a real account, so a row without walletBalance keeps the old fields, and one
  # with nothing readable is logged rather than shown as an empty wallet without a word.
  def free_balance(coin, locked)
    wallet = coin['walletBalance']
    return [wallet.to_d - locked - coin['spotBorrow'].to_d, 0].max if wallet.present?

    legacy = coin['availableToWithdraw'].presence || coin['free'].presence
    Rails.logger.warn("[bybit-balance] no walletBalance or legacy free field for #{coin['coin']}") if legacy.nil?
    legacy.to_d
  end

  def symbol_pair_base(symbol)
    tickers.find_by(ticker: symbol)&.base || symbol
  end

  def symbol_pair_quote(symbol)
    tickers.find_by(ticker: symbol)&.quote || symbol
  end

  # /v5/order/realtime keeps only the latest 500 closed orders per category, and a unified account's
  # closed-order cache empties on a restart; the rest live in /v5/order/history
  # (https://bybit-exchange.github.io/docs/v5/order/open-order). The client answers an empty list with
  # "Order not found" and has no history call, so history is read through its signed GET. An order
  # neither knows is flagged so Bot::StaleOrderResolver can retire it instead of polling it forever.
  def find_order(order_id)
    result = client.get_order(category: 'spot', order_id: order_id)
    return result unless result.failure? && Array(result.errors).include?('Order not found')

    history = client.send(:get_authenticated, '/v5/order/history', { category: 'spot', orderId: order_id })
    return history if history.failure?
    return Result::Failure.new(history.data['retMsg']) unless history.data['retCode'].to_s == '0'

    raw = history.data.dig('result', 'list')&.first
    return Result::Success.new(raw: raw) if raw

    Result::Failure.new("#{name} no longer knows order #{order_id}", data: { not_found: true, missing_ids: [order_id] })
  end

  def parse_error_message(result)
    return unless result.errors.first.present?

    begin
      parsed = JSON.parse(result.errors.first)
      parsed['retMsg'] || parsed['msg']
    rescue StandardError
      nil
    end
  end

  def get_bid_ask_price(ticker)
    cache_key = "exchange_#{id}_bid_ask_price_#{ticker.id}"
    Rails.cache.fetch(cache_key, expires_in: 1.seconds) do
      result = client.orderbook(category: 'spot', symbol: ticker.ticker, limit: 1)
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error, data: result.data) : result
      end

      ret_code = result.data['retCode']
      return Result::Failure.new(result.data['retMsg']) if ret_code != 0

      book = Utilities::Hash.dig_or_raise(result.data, 'result')
      bids = Utilities::Hash.dig_or_raise(book, 'b')
      asks = Utilities::Hash.dig_or_raise(book, 'a')

      formatted = {
        bid: {
          price: bids.first[0].to_d,
          size: bids.first[1].to_d
        },
        ask: {
          price: asks.first[0].to_d,
          size: asks.first[1].to_d
        }
      }
      Result::Success.new(formatted)
    end
  end

  # @param amount [Float] must be a positive number
  # @param amount_type [Symbol] :base or :quote
  # @param side [Symbol] must be either :buy or :sell
  def set_market_order(ticker:, amount:, amount_type:, side:)
    amount = ticker.adjusted_amount(amount: amount, amount_type: amount_type)

    order_settings = {
      category: 'spot',
      symbol: ticker.ticker,
      side: side.to_s.capitalize,
      order_type: 'Market',
      qty: amount.to_d.to_s('F'),
      market_unit: amount_type == :quote ? 'quoteCoin' : 'baseCoin'
    }
    result = client.create_order(**order_settings)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error, data: result.data) : result
    end

    data = {
      order_id: result.data[:order_id]
    }

    Result::Success.new(data)
  end

  # @param amount [Float] must be a positive number
  # @param amount_type [Symbol] :base or :quote
  # @param side [Symbol] must be either :buy or :sell
  # @param price [Float] must be a positive number
  def set_limit_order(ticker:, amount:, amount_type:, side:, price:)
    price = ticker.adjusted_price(price: price)
    return Result::Failure.new("Invalid limit price for #{ticker.ticker}") unless price.to_d.positive?

    # Bybit spot limit orders are base-denominated (qty + price). Convert a :quote amount to
    # base at the adjusted limit price so the order reserves the intended quote rather than
    # shipping the quote figure as a base quantity.
    base_amount = amount_type == :quote ? amount.to_d / price.to_d : amount
    base_amount = ticker.adjusted_amount(amount: base_amount, amount_type: :base)

    order_settings = {
      category: 'spot',
      symbol: ticker.ticker,
      side: side.to_s.capitalize,
      order_type: 'Limit',
      qty: base_amount.to_d.to_s('F'),
      price: price.to_d.to_s('F'),
      time_in_force: 'GTC'
    }
    result = client.create_order(**order_settings)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error, data: result.data) : result
    end

    data = {
      order_id: result.data[:order_id]
    }

    Result::Success.new(data)
  end

  def parse_order_data(order_id, order_data)
    symbol = Utilities::Hash.dig_or_raise(order_data, 'symbol')
    ticker = tickers.find_by(ticker: symbol)
    order_type = parse_order_type(Utilities::Hash.dig_or_raise(order_data, 'orderType'))
    price = Utilities::Hash.dig_or_raise(order_data, 'avgPrice').to_d
    price = Utilities::Hash.dig_or_raise(order_data, 'price').to_d if price.zero?
    price = nil if price.zero?
    side = Utilities::Hash.dig_or_raise(order_data, 'side').downcase.to_sym
    amount = Utilities::Hash.dig_or_raise(order_data, 'qty').to_d
    amount = nil if amount.zero?
    amount_exec = Utilities::Hash.dig_or_raise(order_data, 'cumExecQty').to_d
    quote_amount_exec = Utilities::Hash.dig_or_raise(order_data, 'cumExecValue').to_d
    status = parse_order_status(Utilities::Hash.dig_or_raise(order_data, 'orderStatus'))

    {
      order_id: order_id,
      ticker: ticker,
      price: price,
      amount: amount,
      quote_amount: nil,
      amount_exec: amount_exec,
      quote_amount_exec: quote_amount_exec,
      side: side,
      order_type: order_type,
      error_messages: [order_data['rejectReason'].presence].compact.reject { |r| r == 'EC_NoError' },
      status: status,
      exchange_response: order_data
    }
  end

  def parse_order_type(order_type)
    case order_type
    when 'Market'
      :market_order
    when 'Limit'
      :limit_order
    else
      raise "Unknown #{name} order type: #{order_type}"
    end
  end
end
