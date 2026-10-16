class Exchanges::Bitvavo < Exchange
  COINGECKO_ID = 'bitvavo'.freeze # https://docs.coingecko.com/reference/exchanges-list
  ERRORS = {
    # Bitvavo returns a JSON envelope and its own prose, neither of which contained the strings
    # previously configured here ('Insufficient funds.', 'Invalid API key.', 'Signature invalid.') —
    # so nothing on this venue was ever classified. These are matched against captured bodies:
    #   216 {"errorCode":216,"error":"You do not have sufficient balance to complete this operation."}
    #   305 {"errorCode":305,"error":"No active API key found."}
    #   301 {"errorCode":301,"error":"API Key must be of length 64."}
    # The rest are matched on the code as the body spells it ("errorCode":309), which cannot collide
    # with a number elsewhere in the payload. Codes: https://docs.bitvavo.com/docs/errors/
    insufficient_funds: ['sufficient balance to complete'],
    # 305/306 inactive or unconfirmed key, 301 malformed key, 308/309 a signature the secret did not make.
    invalid_key: ['No active API key found', 'API Key must be of length', '"errorCode":308', '"errorCode":309'],
    # 307 IP not whitelisted, 310 no Trade, 311 no View, 312 no Withdraw: the key itself is good.
    permission_denied: ['"errorCode":307', '"errorCode":310', '"errorCode":311', '"errorCode":312'],
    # 105: over the weight limit, blocked for a minute.
    throttle: ['"errorCode":105', 'Too Many Requests'],
    # 101/400 unknown server error, 107 overloaded, 108 processing issue, 109 timed out (may have
    # succeeded), 111 matching engine down, 419 server unavailable, 304 outside the access window.
    transient: ['"errorCode":101', '"errorCode":107', '"errorCode":108', '"errorCode":109', '"errorCode":111',
                '"errorCode":400', '"errorCode":419', '"errorCode":304'],
    # 304 is an authentication failure: the request never reached the matching engine.
    placement_safe_transient: ['"errorCode":304']
  }.freeze
  ORDER_STATUS_MAP = {
    'new' => :open,
    'partiallyFilled' => :open,
    'filled' => :closed,
    'canceled' => :cancelled,
    'cancelled' => :cancelled,
    'expired' => :cancelled,
    'rejected' => :failed
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
    @client = Honeymaker.client('bitvavo',
                                api_key: api_key&.key,
                                api_secret: api_key&.secret,
                                proxy: ExchangeProxy.for('bitvavo'))
  end

  def get_tickers_info(force: false)
    cache_key = "exchange_#{id}_tickers_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour, force: force) do
      result = client.get_markets
      return result if result.failure?

      result.data.map do |product|
        market = Utilities::Hash.dig_or_raise(product, 'market')
        status = Utilities::Hash.dig_or_raise(product, 'status')
        base, quote = market.split('-')

        {
          ticker: market,
          base: base,
          quote: quote,
          minimum_base_size: product['minOrderInBaseAsset'].to_d,
          minimum_quote_size: product['minOrderInQuoteAsset'].to_d,
          maximum_base_size: nil,
          maximum_quote_size: nil,
          base_decimals: product['quantityDecimals'] || 8,
          quote_decimals: product['notionalDecimals'] || 8,
          price_decimals: Utilities::Number.decimals(Utilities::Hash.dig_or_raise(product, 'tickSize')),
          available: true,
          trading_enabled: status == 'trading'
        }
      end.compact
    end

    Result::Success.new(tickers_info)
  end

  def get_tickers_prices(force: false, symbols: nil)
    cache_key = "exchange_#{id}_prices"
    tickers_prices = Rails.cache.fetch(cache_key, expires_in: 1.minute, force: force) do
      result = client.get_ticker_price
      return result if result.failure?

      result.data.each_with_object({}) do |item, prices_hash|
        ticker = Utilities::Hash.dig_or_raise(item, 'market')
        price = item['price'].to_d
        prices_hash[ticker] = price if price.positive?
      end
    end

    Result::Success.new(tickers_prices)
  end

  def get_balances(asset_ids: nil)
    result = client.get_raw_balance
    return result if result.failure?

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.to_h do |asset_id|
      [asset_id, { free: 0, locked: 0 }]
    end

    result.data.each do |balance_data|
      asset = asset_from_symbol(balance_data['symbol'])
      next unless asset.present?
      next unless asset_ids.include?(asset.id)

      free = Utilities::Hash.dig_or_raise(balance_data, 'available').to_d
      locked = Utilities::Hash.dig_or_raise(balance_data, 'inOrder').to_d

      balances[asset.id] = { free: free, locked: locked }
    end

    Result::Success.new(balances)
  end

  def get_last_price(ticker:, force: false)
    cache_key = "exchange_#{id}_last_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      result = client.get_ticker_price(market: ticker.ticker)
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
    intervals = {
      1.minute => '1m',
      5.minutes => '5m',
      15.minutes => '15m',
      30.minutes => '30m',
      1.hour => '1h',
      4.hours => '4h',
      1.day => '1d',
      3.days => '1d',
      1.week => '1d',
      1.month => '1d'
    }
    interval = intervals[timeframe]

    # Bitvavo answers newest first with the LATEST 1440 candles of the range, so a longer range is
    # read backwards: each page ends (exclusive) where the previous one began.
    limit = 1440
    rows = {}
    end_ms = nil
    loop do
      result = client.get_candles(market: ticker.ticker, interval: interval, start_time: start_at.to_i * 1000,
                                  end_time: end_ms, limit: limit)
      return result if result.failure?

      page = Array(result.data)
      page.each { |candle| rows[candle[0].to_i] = candle }
      oldest = page.map { |candle| candle[0].to_i }.min
      break if page.size < limit || (end_ms && oldest >= end_ms)

      end_ms = oldest
    end
    candles = rows.sort.map do |timestamp, candle|
      [Time.at(timestamp / 1000).utc, candle[1].to_d, candle[2].to_d, candle[3].to_d, candle[4].to_d, candle[5].to_d]
    end

    candles = build_candles_from_candles(candles: candles, timeframe: timeframe) if timeframe.in?([3.days,
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

  def get_order(order_id:)
    # Bitvavo order_id format: "MARKET-orderId" e.g. "BTC-EUR-abc123" (market can contain a dash).
    market, ext_order_id = split_order_id(order_id)
    result = client.get_order(market: market, order_id: ext_order_id)

    if result.failure?
      # errorCode 240 "No active order found" is Bitvavo's order-gone signal. The order may have
      # filled then been dropped → recover the fill from get_trades BEFORE declaring not-found
      # (mirrors Kraken/Hyperliquid). Any OTHER failure (auth/transient/throttle) is propagated
      # untouched so FetchAndUpdateOrderJob's typed-error / retry path is preserved.
      return result unless bitvavo_not_found?(result)

      return recover_order_from_trades(order_id)
    end

    Result::Success.new(parse_order_data(order_id, result.data[:raw]))
  end

  def get_orders(order_ids:)
    orders = {}
    missing = []
    order_ids.each do |order_id|
      result = get_order(order_id: order_id)
      if result.failure?
        # not_found (incl. a 240 with no recoverable fill) → collect under :missing for the
        # open-orders sweep; anything else (transient/throttle/auth) → abort so the job retries.
        if result.data.is_a?(Hash) && result.data[:not_found]
          missing << order_id
          next
        end

        return result
      end

      orders[order_id] = result.data
    end

    # get_order already folds any recovered fill into :orders (status :closed). Only orders absent
    # from BOTH the order endpoint AND get_trades stay missing → StaleOrderResolver (never executed).
    Rails.logger.info("[bitvavo-reconcile] #{missing.size} order(s) missing-from-both: #{missing.join(',')}") if missing.any?

    Result::Success.new(orders: orders, missing: missing)
  end

  # get_trades exhausts Bitvavo's fill history, so an order absent from BOTH the order endpoint
  # and get_trades is confirmed never-executed → Bot::FetchAndUpdateOpenOrdersJob may stop wedging
  # on a young missing id (limit DCA self-heals via missed_quote_amount). Same contract as Kraken/HL.
  def authoritative_missing_orders?
    true
  end

  def cancel_order(order_id:)
    market, ext_order_id = split_order_id(order_id)
    result = client.cancel_order(market: market, order_id: ext_order_id)
    return result if result.failure?

    Result::Success.new(order_id)
  end

  # Decided by Bitvavo's documented errorCodes (https://docs.bitvavo.com/docs/errors/), read from the
  # error body. Every key reads the balance, which needs View. A trading or tracker key then cancels
  # an order that does not exist: 240 "no order found" means the key got past the permission check,
  # 310 means it lacks Trade. The steps ask for both, and Bitvavo serves orders and trades only to a
  # key with both (https://docs.bitvavo.com/docs/rest-api/introduction/#permissions). Everything
  # else — a timeout, a rate limit, an outage, an unknown code — leaves the key unverified.
  KEY_CHECK_ORDER_NOT_FOUND = 240
  KEY_CHECK_REFUSED = [310, 311].freeze # no Trade, no View

  def get_api_key_validity(api_key:)
    temp_client = Honeymaker.client('bitvavo',
                                    api_key: api_key.key,
                                    api_secret: api_key.secret,
                                    proxy: ExchangeProxy.for('bitvavo'))

    probes = [-> { temp_client.get_raw_balance }]
    probes << -> { temp_client.cancel_order(market: 'BTC-EUR', order_id: '00000000-0000-0000-0000-000000000000') } unless api_key.withdrawal?
    probes.each do |probe|
      result = probe.call
      code = response_field(result, 'errorCode')
      next if (result.success? && code.nil?) || code == KEY_CHECK_ORDER_NOT_FOUND
      return Result::Success.new(false) if KEY_CHECK_REFUSED.include?(code) || condemning_invalid_key_error?(result.errors)

      return result.failure? ? result : Result::Failure.new(response_field(result, 'error').presence || "Bitvavo answered errorCode #{code}")
    end
    Result::Success.new(true)
  end

  # A tracker key is checked like a trading one: the ledger reads trades, which need Trade too.
  def get_read_api_key_validity(api_key:)
    get_api_key_validity(api_key: api_key)
  end

  def minimum_amount_logic(order_type:, **)
    # Bitvavo limit orders accept only base amount + price (no amountQuote), so limit
    # orders must be sized in base. Market orders accept either (set_market_order
    # branches amount/amount_quote), so they keep the cheaper-minimum choice.
    order_type == :limit_order ? :base_and_quote_in_base : :base_or_quote
  end

  def withdraw(asset:, amount:, address:, network: nil, address_tag: nil)
    symbol = symbol_from_asset(asset)
    return Result::Failure.new("Unknown symbol for asset #{asset.symbol}") if symbol.blank?

    result = client.withdraw(symbol: symbol, amount: amount.to_d.to_s('F'), address: address,
                             payment_id: address_tag)
    return result if result.failure?

    withdrawal_id = result.data['success'] ? "bitvavo-#{SecureRandom.uuid}" : nil
    Result::Success.new({ withdrawal_id: withdrawal_id })
  end

  def fetch_withdrawal_fees!
    result = Honeymaker.client('bitvavo', proxy: ExchangeProxy.for('bitvavo')).get_assets
    return result if result.failure?

    fees = {}
    chains = {}
    Array(result.data).each do |coin|
      symbol = coin['symbol']
      next if coin['withdrawalFee'].blank?

      fees[symbol] = coin['withdrawalFee']
      chains[symbol] = [{ 'name' => symbol, 'fee' => coin['withdrawalFee'], 'is_default' => true }]
    end

    update_exchange_asset_fees!(fees, chains: chains)
  end

  def get_ledger(api_key:, start_time: nil)
    hm_client = Honeymaker.client('bitvavo', api_key: api_key.key, api_secret: api_key.secret,
                                             proxy: ExchangeProxy.for('bitvavo'))
    start_ms = start_time ? (start_time.to_f * 1000).to_i : nil
    entries = []

    # Deposits and withdrawals. Any failed read fails the whole fetch: a partial Success would let the
    # sync move its watermark past the rows it never got.
    deposits = read_transfers(hm_client, DEPOSITS_PATH, start_ms)
    return deposits if deposits.failure?

    deposits.data.each do |dep|
      # The docs give a status for fiat deposits only; a crypto deposit may come without one.
      next if dep['status'].present? && dep['status'] != 'completed'

      fee = dep['fee'].to_d
      # No record id, and txId is the chain transaction (absent for fiat): the sync keys these on
      # the event itself.
      entries << { entry_type: :deposit, base_currency: dep['symbol'], base_amount: dep['amount'].to_d,
                   quote_currency: nil, quote_amount: nil,
                   fee_currency: fee.positive? ? dep['symbol'] : nil, fee_amount: fee.positive? ? fee : nil,
                   tx_id: nil, group_id: nil, description: nil,
                   transacted_at: Time.at(dep['timestamp'].to_i / 1000.0).utc, raw_data: dep }
    end

    withdrawals = read_transfers(hm_client, WITHDRAWALS_PATH, start_ms)
    return withdrawals if withdrawals.failure?

    withdrawals.data.each do |wd|
      next unless wd['status'] == 'completed'

      entries << { entry_type: :withdrawal, base_currency: wd['symbol'], base_amount: wd['amount'].to_d,
                   quote_currency: nil, quote_amount: nil,
                   fee_currency: wd['symbol'], fee_amount: wd['fee'].to_d,
                   tx_id: nil, group_id: nil, description: nil,
                   transacted_at: Time.at(wd['timestamp'].to_i / 1000.0).utc, raw_data: wd }
    end

    # Trades, per market: every market whose base the account holds or moved in the window, the
    # markets it traded before, and the ones its bots ordered on. Without the balances or the listing,
    # markets would go unread while the fetch succeeded.
    coins = entries.to_set { |entry| entry[:base_currency] }
    balances = hm_client.get_raw_balance
    return balances if balances.failure?

    Array(balances.data).each { |b| coins << b['symbol'] if b['available'].to_d.positive? || b['inOrder'].to_d.positive? }
    listing = hm_client.get_markets
    return listing if listing.failure?

    # A sale that emptied a coin leaves no balance to find its market by, and a bot's buy and sell
    # between two syncs leave no balance, transfer or stored row behind.
    # ponytail: /v2/trades takes one market, so a market traded by hand, bought and sold out between
    # two syncs and never seen before, is still not read. /v2/account/history would find it.
    traded_before = AccountTransaction.where(user_id: api_key.user_id, exchange_id: api_key.exchange_id)
                                      .where(entry_type: %i[buy sell]).distinct
                                      .pluck(:base_currency, :quote_currency)
    traded_before += Transaction.joins(:bot).where(exchange_id: api_key.exchange_id, bots: { user_id: api_key.user_id })
                                .distinct.pluck(:base, :quote)
    traded_before = traded_before.map { |pair| pair.join('-') }
    markets = Array(listing.data).map { |m| m['market'] }
    markets.select { |market| coins.include?(market.split('-').first) || traded_before.include?(market) }.each do |market|
      trades = read_trades(hm_client, market, start_ms)
      return trades if trades.failure?

      base, quote = market.split('-')
      trades.data.each do |trade|
        entries << { entry_type: trade['side'] == 'buy' ? :buy : :sell,
                     base_currency: base, base_amount: trade['amount'].to_d,
                     quote_currency: quote, quote_amount: trade['amount'].to_d * trade['price'].to_d,
                     fee_currency: trade['feeCurrency'], fee_amount: trade['fee'].to_d,
                     tx_id: trade['id'], group_id: nil, description: nil,
                     transacted_at: Time.at(trade['timestamp'].to_i / 1000.0).utc, raw_data: trade }
      end
    end

    Result::Success.new(entries)
  end

  private

  # https://docs.bitvavo.com/docs/rest-api/get-deposit-history/ and …/get-withdrawal-history/.
  # honeymaker 0.12.3 reads /v2/deposit (the deposit address) and GET /v2/withdrawal (405).
  DEPOSITS_PATH = '/v2/depositHistory'.freeze
  WITHDRAWALS_PATH = '/v2/withdrawalHistory'.freeze
  TRANSFERS_PAGE_LIMIT = 1000
  TRADES_PAGE_LIMIT = 1000 # Bitvavo /v2/trades hard max
  private_constant :DEPOSITS_PATH, :WITHDRAWALS_PATH, :TRANSFERS_PAGE_LIMIT, :TRADES_PAGE_LIMIT

  # Every transfer since start_ms (all of them without one). Newest first, at most 1000 a call, and
  # no record id or cursor: each page ends at the oldest timestamp of the one before, plus a
  # millisecond so rows sharing it are read again rather than skipped, and repeats are dropped by
  # content. A full page that brings nothing new fails instead of looping.
  def read_transfers(hm_client, path, start_ms)
    rows = Set.new
    end_ms = (Time.now.to_f * 1000).to_i + 1
    loop do
      result = hm_client.send(:get_signed, path, { start: start_ms || 0, end: end_ms, limit: TRANSFERS_PAGE_LIMIT })
      return result if result.failure?

      page = Array(result.data)
      fresh = page.count { |row| rows.add?(row) }
      break if page.size < TRANSFERS_PAGE_LIMIT
      return Result::Failure.new("#{name} #{path} returned a full page with nothing new") if fresh.zero?

      end_ms = page.map { |row| row['timestamp'].to_i }.min + 1
    end
    Result::Success.new(rows.to_a)
  end

  # "BTC-EUR-abc123" → ["BTC-EUR", "abc123"] (market can contain a dash; orderId can too).
  def split_order_id(order_id)
    parts = order_id.split('-')
    [parts[0..1].join('-'), parts[2..].join('-')]
  end

  # Bitvavo's own errorCode in the JSON body, never the bare 404 a gateway or a wrong path also
  # answers with. Its wording ("No order found. Please be aware…") is not matched: the docs give
  # only the code, and the message this used to require was never the one Bitvavo sends.
  # Any other failure (auth/transient/throttle) is propagated untouched by get_order, so the order
  # is never mistaken for gone and routed into recovery/abandonment.
  def bitvavo_not_found?(result)
    response_field(result, 'errorCode') == KEY_CHECK_ORDER_NOT_FOUND # 240
  end

  # Bitvavo dropped the order (240). Recover any execution from get_trades — authoritative for
  # fills — keyed on this order's id, bounding the lookback by the order's own row (get_trades
  # needs a start_time, and get_order has none) minus a 1h buffer (immediate fill / clock skew).
  # Reads every trade since then (read_trades), so a matching trade can't hide beyond the first page
  # on a high-volume account — this is what lets authoritative_missing_orders? be true. Returns a synthesized :closed when execution
  # is proven, else a distinct not_found signal (so FetchAndUpdateOrderJob's StaleOrderResolver path
  # fires). A transient get_trades failure is PROPAGATED, never degraded to not_found (so the job
  # retries instead of abandoning a real fill).
  def recover_order_from_trades(order_id)
    market, ext_order_id = split_order_id(order_id)
    since = transactions.where(external_id: order_id).minimum(:created_at)
    return not_found_failure(order_id) unless since

    start_ms = ((since - 1.hour).to_f * 1000).to_i
    result = read_trades(client, market, start_ms)
    return result if result.failure?

    trades = result.data.select { |trade| trade['orderId'].to_s == ext_order_id.to_s }
    return not_found_failure(order_id) if trades.empty?

    Result::Success.new(order_data_from_trades(order_id, market, trades))
  end

  # Every trade on `market` from start_ms on (all of them without one). Bitvavo answers newest first,
  # at most 1000 a call. With start and end the range is capped at 24 hours, and start alone is
  # refused ("start and end must be provided together"), but a tradeIdTo cursor alone reaches any
  # age and returns trades strictly older than that id (both checked live on the public trades
  # endpoint, which takes the same parameters). So each page continues before the oldest id of the
  # one before, until a short page or one that reaches past start_ms. Repeats are dropped by id, and
  # a full page with nothing new fails rather than loops. A failed page fails the whole read.
  def read_trades(hm_client, market, start_ms)
    trades = {}
    cursor = nil
    loop do
      result = hm_client.get_trades(market: market, limit: TRADES_PAGE_LIMIT, trade_id_to: cursor)
      return result if result.failure?

      page = Array(result.data)
      fresh = page.count { |trade| trades[trade['id']] ? false : (trades[trade['id']] = trade) }
      break if page.size < TRADES_PAGE_LIMIT || (start_ms && page.map { |trade| trade['timestamp'].to_i }.min < start_ms)
      return Result::Failure.new("#{name} trades on #{market} returned a full page with nothing new") if fresh.zero?

      cursor = page.last['id'] # the oldest: pages come in trade sequence, newest first
    end
    Result::Success.new(trades.values.select { |trade| start_ms.nil? || trade['timestamp'].to_i >= start_ms })
  end

  def not_found_failure(order_id)
    Result::Failure.new("Bitvavo did not return data for order #{order_id}",
                        data: { not_found: true, missing_ids: [order_id] })
  end

  def order_data_from_trades(order_id, market, trades)
    amount_exec = trades.sum(BigDecimal('0')) { |t| t['amount'].to_d }
    quote_amount_exec = trades.sum(BigDecimal('0')) { |t| t['amount'].to_d * t['price'].to_d }
    first = trades.first
    {
      order_id: order_id,
      ticker: tickers.find_by(ticker: market),
      price: amount_exec.positive? ? (quote_amount_exec / amount_exec) : nil,
      amount: nil,
      quote_amount: nil,
      amount_exec: amount_exec,
      quote_amount_exec: quote_amount_exec,
      side: first['side'].to_s.downcase.to_sym,
      order_type: :limit_order, # DCA waiting orders are limit; trades don't carry orderType. See D3.
      error_messages: [],
      status: :closed,
      exchange_response: { 'trades' => trades }
    }
  end

  def get_bid_ask_price(ticker)
    cache_key = "exchange_#{id}_bid_ask_price_#{ticker.id}"
    Rails.cache.fetch(cache_key, expires_in: 1.seconds) do
      result = client.get_ticker_book(market: ticker.ticker)
      return result if result.failure?

      formatted = {
        bid: {
          price: Utilities::Hash.dig_or_raise(result.data, 'bid').to_d,
          size: Utilities::Hash.dig_or_raise(result.data, 'bidSize').to_d
        },
        ask: {
          price: Utilities::Hash.dig_or_raise(result.data, 'ask').to_d,
          size: Utilities::Hash.dig_or_raise(result.data, 'askSize').to_d
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
      market: ticker.ticker,
      side: side.to_s,
      order_type: 'market',
      amount: amount_type == :base ? amount.to_d.to_s('F') : nil,
      amount_quote: amount_type == :quote ? amount.to_d.to_s('F') : nil
    }
    result = client.place_order(**order_settings)
    return result if result.failure?

    # Honeymaker's client#place_order returns { order_id: "MARKET-<id>", raw: {...} }
    data = {
      order_id: Utilities::Hash.dig_or_raise(result.data, :order_id)
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

    # Bitvavo limit orders are base-denominated (base amount + price; no amountQuote).
    # Convert a :quote amount to base at the adjusted limit price so the order reserves
    # the intended quote rather than shipping the quote figure as a base quantity.
    base_amount = amount_type == :quote ? amount.to_d / price.to_d : amount
    base_amount = ticker.adjusted_amount(amount: base_amount, amount_type: :base)

    order_settings = {
      market: ticker.ticker,
      side: side.to_s,
      order_type: 'limit',
      amount: base_amount.to_d.to_s('F'),
      price: price.to_d.to_s('F')
    }
    result = client.place_order(**order_settings)
    return result if result.failure?

    # Honeymaker's client#place_order returns { order_id: "MARKET-<id>", raw: {...} }
    data = {
      order_id: Utilities::Hash.dig_or_raise(result.data, :order_id)
    }

    Result::Success.new(data)
  end

  def parse_order_data(order_id, order_data)
    market = Utilities::Hash.dig_or_raise(order_data, 'market')
    ticker = tickers.find_by(ticker: market)
    order_type = parse_order_type(Utilities::Hash.dig_or_raise(order_data, 'orderType'))
    price = order_data['price'].to_d
    side = Utilities::Hash.dig_or_raise(order_data, 'side').downcase.to_sym
    amount = order_data['amount'].to_d
    amount = nil if amount.zero?
    amount_quote = order_data['amountQuote'].to_d
    amount_quote = nil if amount_quote.zero?
    amount_exec = order_data['filledAmount'].to_d
    quote_amount_exec = order_data['filledAmountQuote'].to_d
    # `price` is the limit set on the order; what traded is the average of the fills.
    price = quote_amount_exec / amount_exec if quote_amount_exec.positive? && amount_exec.positive?
    price = nil if price.zero?
    status = parse_order_status(Utilities::Hash.dig_or_raise(order_data, 'status'))

    {
      order_id: order_id,
      ticker: ticker,
      price: price,
      amount: amount,
      quote_amount: amount_quote,
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
    when 'market'
      :market_order
    when 'limit'
      :limit_order
    else
      raise "Unknown #{name} order type: #{order_type}"
    end
  end
end
