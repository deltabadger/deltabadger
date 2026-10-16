class Exchanges::Bingx < Exchange
  COINGECKO_ID = 'bingx'.freeze # https://docs.coingecko.com/reference/exchanges-list
  # Codes and wording from https://bingx-api.github.io/docs/#/en-us/common/error-code (and the samples
  # on the place-order page); 100413 captured from a live probe with a bogus key.
  ERRORS = {
    # 100202 "Insufficient assets, please verify the assets status".
    insufficient_funds: ['Insufficient assets', 'Insufficient balance', 'insufficient balance'],
    # 100413 "Incorrect apiKey", 100412 "Null signature", 100001 "Signature verification failed".
    invalid_key: ['Incorrect apiKey', 'Invalid Api-Key ID', 'Signature verification failed', 'Null signature'],
    # 100004 a key without the endpoint's permission, 100419 a request from outside the key's IP list:
    # the key itself is good, so neither may condemn it.
    permission_denied: ['API key was created without the permission', 'IP does not match IP whitelist'],
    # HTTP 429, and 100410 "rate limitation".
    throttle: ['Too Many Requests', 'rate limitation'],
    # 100500 and 100410 "The current system is busy", 100421 "Null timestamp or timestamp mismatch".
    transient: ['The current system is busy', 'Internal system error', 'timestamp mismatch']
  }.freeze
  ORDER_STATUS_MAP = {
    'NEW' => :open,
    'PARTIALLY_FILLED' => :open,
    'PENDING' => :open,
    'FILLED' => :closed,
    'CANCELED' => :cancelled,
    'EXPIRED' => :cancelled,
    'REJECTED' => :failed,
    'FAILED' => :failed
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
    @client = Honeymaker.client('bingx',
                                api_key: api_key&.key,
                                api_secret: api_key&.secret,
                                proxy: ExchangeProxy.for('bingx'))
  end

  def get_tickers_info(force: false)
    cache_key = "exchange_#{id}_tickers_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour, force: force) do
      result = client.get_symbols
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error, data: result.data) : result
      end

      return Result::Failure.new(result.data['msg']) if result.data['code'].to_i != 0

      items = Utilities::Hash.dig_or_raise(result.data, 'data', 'symbols')
      items.filter_map do |product|
        ticker = Utilities::Hash.dig_or_raise(product, 'symbol')
        parts = ticker.split('-')
        next unless parts.size == 2

        # minQty and maxQty are deprecated and no longer track the price (BTC-USDT's minQty is worth
        # ~15 USDT against a 0.5 USDT minNotional): the minimum is minNotional alone
        # (https://bingx-api.github.io/docs/#/en-us/spot/market-api.html#Spot%20trading%20symbols).
        {
          ticker: ticker,
          base: parts[0],
          quote: parts[1],
          minimum_base_size: 0,
          minimum_quote_size: product['minNotional'].to_d,
          maximum_base_size: nil,
          maximum_quote_size: product['maxNotional'].to_d,
          base_decimals: Utilities::Number.decimals(product['stepSize']),
          quote_decimals: Utilities::Number.decimals(product['tickSize']),
          price_decimals: Utilities::Number.decimals(product['tickSize']),
          available: true,
          trading_enabled: product['status'].to_i == 1
        }
      end
    end

    Result::Success.new(tickers_info)
  end

  def get_tickers_prices(force: false, symbols: nil)
    cache_key = "exchange_#{id}_prices"
    tickers_prices = Rails.cache.fetch(cache_key, expires_in: 1.minute, force: force) do
      result = client.get_ticker
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error, data: result.data) : result
      end

      return Result::Failure.new(result.data['msg']) if result.data['code'].to_i != 0

      items = Utilities::Hash.dig_or_raise(result.data, 'data')
      items.each_with_object({}) do |item, prices_hash|
        ticker = Utilities::Hash.dig_or_raise(item, 'symbol')
        price = Utilities::Hash.dig_or_raise(item, 'lastPrice').to_d
        prices_hash[ticker] = price
      end
    end

    Result::Success.new(tickers_prices)
  end

  def get_balances(asset_ids: nil)
    result = client.get_raw_balances
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error, data: result.data) : result
    end

    return Result::Failure.new(result.data['msg']) if result.data['code'].to_i != 0

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.to_h do |asset_id|
      [asset_id, { free: 0, locked: 0 }]
    end

    raw_balances = Utilities::Hash.dig_or_raise(result.data, 'data', 'balances')
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
      result = client.get_ticker(symbol: ticker.ticker)
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error, data: result.data) : result
      end

      return Result::Failure.new(result.data['msg']) if result.data['code'].to_i != 0

      items = Utilities::Hash.dig_or_raise(result.data, 'data')
      item = items.is_a?(Array) ? items.first : items
      price = Utilities::Hash.dig_or_raise(item, 'lastPrice').to_d
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

    # BingX's kline endpoint only serves the most-recent `limit` candles: older start_times
    # return empty and paging further back (via an older end_time) also returns empty. So a
    # forward-paging loop either bailed on the empty old-start window (leaving "% from ATH"
    # bots frozen) or duplicated the recent window. Fetch that single most-recent page and
    # trim it to the requested range instead. ATH over this window is the best obtainable;
    # the stored `ath` keeps climbing via incremental-max on later ticks.
    #
    # BingX anchors spot candles to UTC+8 unless asked for timeZone=0, so a daily candle ran from
    # 16:00 to 16:00 UTC. honeymaker's get_klines has no timeZone, hence its public request directly.
    limit = 1000
    result = client.send(:get_public, '/openApi/spot/v1/market/kline',
                         { symbol: ticker.ticker, interval: interval, limit: limit, timeZone: 0 })
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error, data: result.data) : result
    end

    items = result.data.is_a?(Hash) ? (result.data['data'] || []) : (result.data || [])
    start_at_i = start_at.to_i
    candles = items.filter_map do |candle|
      time = candle[0].to_i / 1000
      next if time < start_at_i

      [
        Time.at(time).utc,
        candle[1].to_d,
        candle[2].to_d,
        candle[3].to_d,
        candle[4].to_d,
        candle[5].to_d
      ]
    end.sort_by { |c| c[0] } # BingX can return newest-first

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
    symbol, ext_order_id = split_order_id(order_id)
    result = client.get_order(symbol: symbol, order_id: ext_order_id)
    if result.failure?
      # Flagged so get_orders lists it under missing and Bot::StaleOrderResolver can retire it,
      # instead of one gone order failing every poll of the bot's other orders.
      if order_gone?(result)
        return Result::Failure.new("#{name} no longer knows order #{order_id}",
                                   data: { not_found: true, missing_ids: [order_id] })
      end

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
    symbol, ext_order_id = split_order_id(order_id)
    result = client.cancel_order(symbol: symbol, order_id: ext_order_id)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error, data: result.data) : result
    end
    # BingX refuses a cancel with HTTP 200 and a non-zero code.
    code = result.data['code']
    return Result::Failure.new(result.data['msg'].presence || "BingX API error #{code}", data: result.data) if code.to_i != 0

    Result::Success.new(order_id)
  end

  # Decided by BingX's documented codes (from BingX's own API reference repo), read from the envelope or
  # an error body. The trading probe cancels an order that does not exist: 100404 means the key got past
  # the permission check. A missing code is no longer read as 0 ("ok") — it and any unknown code leave
  # the key unverified.
  # 100404 counts only when its message is about the order: BingX also answers it for a gateway path
  # it does not know. 100400 never counts — it also means a missing parameter or an unknown symbol.
  KEY_CHECK_ORDER_NOT_FOUND = 100_404
  KEY_CHECK_REJECTED = [100_413, 100_004].freeze # invalid key, missing permission

  def get_api_key_validity(api_key:)
    temp_client = Honeymaker.client('bingx',
                                    api_key: api_key.key,
                                    api_secret: api_key.secret,
                                    proxy: ExchangeProxy.for('bingx'))

    result = if api_key.withdrawal?
               temp_client.get_raw_balances
             else
               temp_client.cancel_order(symbol: 'BTC-USDT', order_id: '0')
             end

    raw_code = response_field(result, 'code')
    code = Integer(raw_code, exception: false) unless raw_code.nil?
    message = response_field(result, 'msg').to_s
    return Result::Success.new(true) if code&.zero?
    return Result::Success.new(true) if !api_key.withdrawal? && code == KEY_CHECK_ORDER_NOT_FOUND && message.match?(/order/i)
    return Result::Success.new(false) if code.in?(KEY_CHECK_REJECTED)
    return Result::Success.new(false) if ERRORS[:invalid_key].any? { |msg| message.include?(msg) }
    return check_bingx_api_key_error(result) if result.failure?

    Result::Failure.new(message.presence || "BingX answered code #{raw_code.inspect}")
  end

  def minimum_amount_logic(order_type:, **)
    # Limit orders take a base quantity only; market orders take either.
    order_type == :limit_order ? :base_and_quote_in_base : :base_and_quote
  end

  def withdraw(asset:, amount:, address:, network: nil, address_tag: nil)
    symbol = symbol_from_asset(asset)
    return Result::Failure.new("Unknown symbol for asset #{asset.symbol}") if symbol.blank?

    # Use provided network or determine the default
    network_name = network
    if network_name.blank?
      coin_result = client.get_all_coins_info
      return coin_result if coin_result.failure?

      coins = coin_result.data['data'] || []
      coin_data = coins.find { |c| c['coin'] == symbol }
      return Result::Failure.new("No coin data found for #{symbol} on BingX") if coin_data.blank?

      networks = coin_data['networkList'] || []
      default_network = networks.find { |n| n['isDefault'] == true } || networks.first
      return Result::Failure.new("No network found for #{symbol} on BingX") if default_network.blank?

      network_name = default_network['network']
    end

    result = client.withdraw(coin: symbol, address: address, amount: amount.to_d.to_s('F'),
                             network: network_name, tag: address_tag)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error, data: result.data) : result
    end

    return Result::Failure.new(result.data['msg']) if result.data['code'].to_i != 0

    withdrawal_id = result.data.dig('data', 'id')
    Result::Success.new({ withdrawal_id: withdrawal_id })
  end

  def fetch_withdrawal_fees!
    with_authenticated_fee_client('bingx', 'PROXY_BINGX') do |hm_client|
      result = hm_client.get_all_coins_info
      return result if result.failure?

      fees = {}
      chain_data = {}
      coins = result.data['data'] || []
      coins.each do |coin|
        symbol = coin['coin']
        networks = coin['networkList'] || []
        default_net = networks.find { |n| n['isDefault'] == true } || networks.first
        next unless default_net

        fees[symbol] = default_net['withdrawFee']
        chain_data[symbol] = networks.map do |n|
          { 'name' => n['network'], 'fee' => n['withdrawFee'], 'is_default' => n['isDefault'] == true }
        end
      end

      update_exchange_asset_fees!(fees, chains: chain_data)
    end
  end

  def get_ledger(api_key:, start_time: nil)
    hm_client = Honeymaker.client('bingx', api_key: api_key.key, api_secret: api_key.secret,
                                           proxy: ExchangeProxy.for('bingx'))
    entries = []

    # Deposits and withdrawals. Any failed read fails the whole fetch: a partial Success would let
    # the sync move its watermark past the rows it never got.
    failed = each_transfer(hm_client, DEPOSITS_PATH, start_time) do |dep|
      next unless dep['status'].to_i == 1 # completed

      # No record id: txId is the chain transaction that credited this deposit.
      entries << { entry_type: :deposit, base_currency: transfer_coin(dep), base_amount: dep['amount'].to_d,
                   quote_currency: nil, quote_amount: nil, fee_currency: nil, fee_amount: nil,
                   tx_id: dep['txId'].presence, group_id: nil, description: nil,
                   transacted_at: Time.at(dep['insertTime'].to_i / 1000.0).utc, raw_data: dep }
    end
    return failed if failed

    failed = each_transfer(hm_client, WITHDRAWALS_PATH, start_time) do |wd|
      next unless wd['status'].to_i == 6 # completed

      coin = transfer_coin(wd)
      # The record's own id: withdrawals batched into one on-chain transaction share its txId, and the
      # sync keeps one row per tx_id.
      entries << { entry_type: :withdrawal, base_currency: coin, base_amount: wd['amount'].to_d,
                   quote_currency: nil, quote_amount: nil,
                   fee_currency: coin, fee_amount: wd['transactionFee']&.to_d,
                   tx_id: wd['id'].to_s.presence, group_id: nil, description: nil,
                   transacted_at: transfer_time(wd['applyTime']), raw_data: wd }
    end
    return failed if failed

    # Trades, per pair: the coins held now or moved in the window, on every pair they are the base of,
    # and the pairs the account traded before.
    # Without the balances or the listing, pairs would go unread while the fetch succeeded.
    coins = entries.to_set { |e| e[:base_currency] }
    balances = hm_client.get_raw_balances
    return balances if balances.failure?
    return Result::Failure.new("#{name} ledger read failed: #{balances.data['msg']}") if balances.data['code'].to_i != 0

    Array(balances.data.dig('data', 'balances')).each do |bal|
      coins << bal['asset'] if bal['free'].to_d.positive? || bal['locked'].to_d.positive?
    end
    listing = hm_client.get_symbols
    return listing if listing.failure?
    return Result::Failure.new("#{name} ledger read failed: #{listing.data['msg']}") if listing.data['code'].to_i != 0

    symbols = Array(listing.data.dig('data', 'symbols')).map { |s| s['symbol'] }
    trades_from = [start_time, TRADES_REACH.ago].compact.max
    lost = lost_bot_fills(api_key, trades_from) if start_time
    return lost if lost

    # A pair traded before counts too: the sale that emptied a coin leaves no balance to find it by.
    # So does every pair the account's bots ordered on here, synced or not: a bot's buy and sell
    # between two syncs leave no balance, transfer or stored row behind.
    # ponytail: myTrades takes one symbol, so a pair traded by hand, bought and sold out between two
    # syncs and never seen before, is still not read. Only a venue-wide fills endpoint would find it.
    traded_before = AccountTransaction.where(user_id: api_key.user_id, exchange_id: api_key.exchange_id)
                                      .where(entry_type: %i[buy sell]).distinct
                                      .pluck(:base_currency, :quote_currency)
    traded_before += Transaction.joins(:bot).where(exchange_id: api_key.exchange_id, bots: { user_id: api_key.user_id })
                                .distinct.pluck(:base, :quote)
    traded_before = traded_before.map { |pair| pair.join('-') }
    symbols.select { |symbol| coins.include?(symbol.split('-').first) || traded_before.include?(symbol) }.each do |symbol|
      trades = read_trades(hm_client, symbol, (trades_from.to_f * 1000).to_i, (Time.now.to_f * 1000).to_i)
      return trades unless trades.is_a?(Array)

      trades.each do |fill|
        base, quote = symbol.split('-')
        entries << { entry_type: fill['isBuyer'] ? :buy : :sell,
                     base_currency: base, base_amount: fill['qty'].to_d,
                     quote_currency: quote, quote_amount: fill['quoteQty'].to_d,
                     fee_currency: fill['commissionAsset'], fee_amount: fill['commission'].to_d.abs,
                     tx_id: fill['id'].to_s, group_id: nil, description: nil,
                     transacted_at: Time.at(fill['time'].to_i / 1000.0).utc, raw_data: fill }
      end
    end

    Result::Success.new(entries)
  end

  private

  # honeymaker's get_trade_fills asks /openApi/spot/v1/trade/fills, which BingX answers with 100400
  # "this api is not exist". myTrades (https://bingx-api.github.io/docs/#/en-us/spot/trade-api.html#Query%20transaction%20details)
  # wants a symbol, "can only check data within the past 7 days", answers 500 trades at most, documents
  # no paging, and falls back to the past 24 hours when the range is invalid — so the range is kept
  # inside the 7 days, with a margin for clock skew.
  TRADES_PATH = '/openApi/spot/v1/trade/myTrades'.freeze
  TRADES_PAGE = 500
  TRADES_REACH = 7.days - 5.minutes
  # 100204 "Requested data not found" (also its answer for a range with no trades) and 100490
  # "spot trading pair is offline": no trades to give.
  NO_TRADES_CODES = [100_204, 100_490].freeze

  # A sync that comes back after BingX's 7 days cannot read the trades in between, and skipping them
  # would leave the tracker short with nothing said. Rails knows its own bots' fills, so the gap is
  # provable for those: a fill after the watermark (it would have moved the watermark had it been
  # read) and older than the reach, on a coin with no synced or imported trade in that span, is lost.
  # The fetch then fails — the tracker banner and the tax report show it — until the user imports
  # those trades from a file. A gap holding only trades placed by hand cannot be seen.
  def lost_bot_fills(api_key, reach_start)
    since = api_key.last_synced_at
    return if since.nil? || since >= reach_start

    gap = since...reach_start
    fills = Transaction.joins(:bot).where(exchange_id: api_key.exchange_id, bots: { user_id: api_key.user_id })
                       .submitted.where('amount_exec > 0').where(created_at: gap)
    covered = AccountTransaction.where(user_id: api_key.user_id, exchange_id: api_key.exchange_id,
                                       entry_type: %i[buy sell], transacted_at: gap).distinct.pluck(:base_currency)
    lost = fills.where.not(base: covered)
    return unless lost.exists?

    Result::Failure.new("#{name} serves trades from the past 7 days only: your bots' #{lost.distinct.pluck(:base).sort.join(', ')} " \
                        "trades between #{since.to_date} and #{reach_start.to_date} can no longer be read. " \
                        'Import them from a file to resume the sync.')
  end

  # The trades of `symbol` in [from, to] (epoch ms), or the failure that stopped the read. A full page
  # may have cut trades off, so its range is split in two halves that share the midpoint (a trade
  # stamped on it comes back in both and is kept once, by id) and both are read.
  def read_trades(hm_client, symbol, from, to)
    result = hm_client.send(:get_signed, TRADES_PATH, { symbol: symbol, startTime: from, endTime: to, limit: TRADES_PAGE })
    return result if result.failure?

    code = result.data['code'].to_i
    return [] if NO_TRADES_CODES.include?(code)
    return Result::Failure.new("#{name} ledger read failed: #{code} #{result.data['msg']}") if code != 0

    trades = Array(result.data.dig('data', 'fills'))
    return trades if trades.size < TRADES_PAGE
    return Result::Failure.new("#{name} ledger read failed: over #{TRADES_PAGE} #{symbol} trades in #{from}..#{to}") if to - from < 2

    mid = (from + to) / 2
    older = read_trades(hm_client, symbol, from, mid)
    return older unless older.is_a?(Array)

    newer = read_trades(hm_client, symbol, mid, to)
    newer.is_a?(Array) ? (older + newer).uniq { |trade| trade['id'] } : newer
  end

  # honeymaker's deposit_history and withdraw_history ask /openApi/wallets/v1/capital/..., which BingX
  # answers with 100400 "this api is not exist"; these are the documented paths
  # (https://bingx-api.github.io/docs/#/en-us/common/wallet-api.html). Both take startTime/endTime and
  # page by offset, 1000 rows at most. Their range cap is undocumented: they are read in 90-day
  # windows, the cap of the Binance endpoints they copy, from start_time or BingX's opening.
  DEPOSITS_PATH = '/openApi/api/v3/capital/deposit/hisrec'.freeze
  WITHDRAWALS_PATH = '/openApi/api/v3/capital/withdraw/history'.freeze
  TRANSFER_PAGE = 1000
  TRANSFER_WINDOW = 90.days
  TRANSFERS_FROM = Time.utc(2018, 1, 1)
  # 100004: a key without the wallet read permission has no transfers to give.
  NO_PERMISSION_CODE = 100_004

  # Yields each transfer record from start_time (or BingX's opening) to now; returns the failure that
  # stopped the read, or nil.
  def each_transfer(hm_client, path, start_time, &)
    seen = Set.new
    each_ledger_window(start_time || TRANSFERS_FROM, TRANSFER_WINDOW) do |from, to|
      offset = 0
      loop do
        result = hm_client.send(:get_signed, path, { startTime: from, endTime: to, offset: offset, limit: TRANSFER_PAGE })
        return result if result.failure?

        body = result.data
        if body.is_a?(Hash)
          return nil if body['code'].to_i == NO_PERMISSION_CODE
          return Result::Failure.new("#{name} ledger read failed: #{body['code']} #{body['msg']}") if body['code'].to_i != 0
        end

        rows = body.is_a?(Array) ? body : Array(body['data'])
        # Neighbouring windows share their edge, so a record stamped on it comes back twice.
        fresh = rows.reject { |row| seen.include?(row) }
        fresh.each { |row| seen << row }
        fresh.each(&)
        break if rows.size < TRANSFER_PAGE
        return Result::Failure.new("#{name} ledger read failed: a full page of #{path} brought nothing new") if fresh.empty?

        offset += rows.size
      end
    end
    nil
  end

  # Yields [from, to] epoch-ms windows of at most `window`, from `from` to now. Each starts where the
  # last ended, so a row on the edge is read either way.
  def each_ledger_window(from, window)
    window_start = (from.to_f * 1000).to_i
    now = (Time.now.to_f * 1000).to_i
    while window_start < now
      window_end = [window_start + window.in_milliseconds.to_i, now].min
      yield window_start, window_end
      window_start = window_end
    end
  end

  # BingX names some coins with their network ("USDTTRC20" on network "TRC20").
  def transfer_coin(row)
    coin = row['coin'].to_s
    network = row['network'].to_s
    network.present? && coin != network && coin.end_with?(network) ? coin.delete_suffix(network) : coin
  end

  # applyTime is a date string in the docs ("2023-12-14T04:05:02.000+08:00"); epoch ms is read too.
  def transfer_time(value)
    value.to_s.match?(/\A\d+\z/) ? Time.at(value.to_i / 1000.0).utc : Time.parse(value.to_s).utc
  end

  # The order lookup's answer for an order BingX does not have: 100404 "Order does not exist" ("order
  # not exist" live), or 100400 with that message (https://bingx-api.github.io/docs/#/en-us/common/error-code).
  # Neither code counts alone: 100404 is also the gateway's answer for a path it does not know, and
  # 100400 a bad parameter — calling a live order gone would let the stale-order sweep abandon it.
  def order_gone?(result)
    message = result.errors.join(' ')
    code = response_field(result, 'code')&.to_i || message[/BingX API error (\d+)/, 1]&.to_i
    [KEY_CHECK_ORDER_NOT_FOUND, 100_400].include?(code) && message.match?(/order (does )?not exist/i)
  end

  def check_bingx_api_key_error(result)
    error = parse_error_message(result)
    if error.present? && ERRORS[:invalid_key].any? { |msg| error.include?(msg) }
      Result::Success.new(false)
    else
      result
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

  def get_bid_ask_price(ticker)
    cache_key = "exchange_#{id}_bid_ask_price_#{ticker.id}"
    Rails.cache.fetch(cache_key, expires_in: 1.seconds) do
      result = client.get_depth(symbol: ticker.ticker, limit: 1)
      if result.failure?
        error = parse_error_message(result)
        return error.present? ? Result::Failure.new(error, data: result.data) : result
      end

      return Result::Failure.new(result.data['msg']) if result.data['code'].to_i != 0

      book = Utilities::Hash.dig_or_raise(result.data, 'data')
      bids = Utilities::Hash.dig_or_raise(book, 'bids')
      asks = Utilities::Hash.dig_or_raise(book, 'asks')

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
      symbol: ticker.ticker,
      side: side.to_s.upcase,
      type: 'MARKET',
      quote_order_qty: amount_type == :quote ? amount.to_d.to_s('F') : nil,
      quantity: amount_type == :base ? amount.to_d.to_s('F') : nil
    }
    result = client.place_order(**order_settings)
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

    # A BingX limit order takes a base quantity only: convert a quote amount at the limit price.
    amount = amount.to_d / price.to_d if amount_type == :quote
    amount = ticker.adjusted_amount(amount: amount, amount_type: :base)

    order_settings = {
      symbol: ticker.ticker,
      side: side.to_s.upcase,
      type: 'LIMIT',
      time_in_force: 'GTC',
      price: price.to_d.to_s('F'),
      quantity: amount.to_d.to_s('F')
    }
    result = client.place_order(**order_settings)
    if result.failure?
      error = parse_error_message(result)
      return error.present? ? Result::Failure.new(error, data: result.data) : result
    end

    data = {
      order_id: result.data[:order_id]
    }

    Result::Success.new(data)
  end

  # "BTC-USDT-123" -> ["BTC-USDT", "123"]: the symbol has a dash, the venue id has none.
  def split_order_id(order_id)
    symbol, _, ext_order_id = order_id.rpartition('-')
    [symbol, ext_order_id]
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
    # `price` is the limit on a limit order and 0 on a market one; whatever filled is priced at what
    # it filled at, which for a limit that crossed the book is better than the limit.
    price = Utilities::Hash.dig_or_raise(order_data, 'price').to_d
    if quote_amount_exec.present? &&
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
end
