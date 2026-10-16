class Exchanges::Hyperliquid < Exchange
  COINGECKO_ID = 'hyperliquid-spot'.freeze
  ERRORS = {
    # 'Insufficient spot balance' is the wording observed in production (2026-08-16): the venue
    # says "Insufficient spot balance asset=10266", so neither of the two generic entries below
    # is a substring of it. Every rejection also arrives behind a "Hyperliquid order failed: "
    # prefix (see set_order), which is why exact matching could never have worked here.
    insufficient_funds: ['Insufficient spot balance', 'Insufficient balance', 'Not enough balance'],
    # The venue recovers the signer from the signature, so an agent wallet that was never approved,
    # has expired, or a secret that is not the agent's all read the same way:
    #   {"status":"err","response":"User or API Wallet 0x… does not exist."}
    # Hyperliquid has no API-key vocabulary; the strings that stood here never occur.
    invalid_key: ['User or API Wallet'],
    # Address-based action limit (https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/rate-limits-and-user-limits):
    #   {"status":"err","response":"Too many cumulative requests sent (37986 > 10436) for cumulative volume traded …"}
    # and the 1200-weight IP limit, an HTTP 429 (hyperliquid-rb: "HTTP 429: …"; Faraday: "status 429").
    throttle: ['Too many cumulative requests sent', 'HTTP 429', 'status 429']
  }.freeze

  # Hyperliquid price rule: at most SIGNIFICANT_FIGURES significant figures and,
  # for spot, at most (SPOT_MAX_DECIMALS - szDecimals) decimal places.
  SIGNIFICANT_FIGURES = 5
  SPOT_MAX_DECIMALS   = 8
  ORDER_PLACEMENT_AVAILABLE = Gem.loaded_specs.key?('hyperliquid-rb')
  ORDER_PLACEMENT_UNAVAILABLE_ERROR = 'Hyperliquid order placement is not available on this installation'.freeze

  include Exchange::Dryable

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

  # Bundler activates specifications without loading their feature files. This distinguishes an
  # installed hyperliquid-rb (whose Hyperliquid constant is still intentionally undefined) from
  # the Windows bundle, where the conditional Gemfile entry is absent altogether.
  def order_placement_available?
    ORDER_PLACEMENT_AVAILABLE
  end

  def set_client(api_key: nil)
    @api_key = api_key
    @client = Honeymaker.client('hyperliquid',
                                api_key: api_key&.key,
                                api_secret: api_key&.secret,
                                proxy: ExchangeProxy.for('hyperliquid'))
  end

  def get_tickers_info(force: false)
    cache_key = "exchange_#{id}_tickers_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour, force:) do
      result = client.spot_meta
      return result if result.failure?

      tokens = result.data['tokens']
      universe = result.data['universe']
      token_map = tokens.to_h { |t| [t['index'], t] }

      universe.map do |pair|
        base_token = token_map[pair['tokens'][0]]
        quote_token = token_map[pair['tokens'][1]]
        next unless base_token && quote_token

        {
          ticker: pair['name'],
          base: base_token['name'],
          quote: quote_token['name'],
          minimum_base_size: 0,
          minimum_quote_size: 10, # Hyperliquid hard spot floor: orders below 10 USDC are rejected

          maximum_base_size: nil,
          maximum_quote_size: nil,
          base_decimals: base_token['szDecimals'] || 0,
          quote_decimals: 2, # USDC is always 2 decimals
          price_decimals: 5, # Hyperliquid uses up to 5 significant figures for prices
          available: true,
          trading_enabled: true
        }
      end.compact
    end

    Result::Success.new(tickers_info)
  end

  def get_tickers_prices(force: false, symbols: nil)
    cache_key = "exchange_#{id}_prices"
    tickers_prices = Rails.cache.fetch(cache_key, expires_in: 1.minute, force:) do
      result = client.all_mids
      return result if result.failure?

      # Filter to only tickers we have in our DB
      available_tickers = tickers.available.pluck(:ticker)
      result.data.select { |ticker, _| ticker.in?(available_tickers) }
            .transform_values(&:to_d)
    end

    Result::Success.new(tickers_prices)
  end

  def get_balances(asset_ids: nil)
    result = client.spot_balances
    return result if result.failure?

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.to_h do |asset_id|
      [asset_id, { free: 0, locked: 0 }]
    end

    balance_list = result.data['balances'] || []
    balance_list.each do |balance|
      asset = asset_from_symbol(balance['coin'])
      next unless asset.present?
      next unless asset_ids.include?(asset.id)

      total = balance['total'].to_d
      hold = (balance['hold'] || '0').to_d
      free = total - hold
      balances[asset.id] = { free:, locked: hold }
    end

    Result::Success.new(balances)
  end

  def get_last_price(ticker:, force: false)
    cache_key = "exchange_#{id}_last_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force:) do
      result = client.all_mids
      return result if result.failure?

      price = result.data[ticker.ticker]&.to_d
      raise "No price available for #{ticker.ticker}" if price.nil? || price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_bid_price(ticker:, force: false)
    cache_key = "exchange_#{id}_bid_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force:) do
      result = get_l2_book(ticker)
      return result if result.failure?

      price = result.data.dig(:bid, :price)
      return Result::Failure.new("Hyperliquid has no bids for #{ticker.ticker}") if price.nil?
      raise "Wrong bid price for #{ticker.ticker}: #{price}" if price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_ask_price(ticker:, force: false)
    cache_key = "exchange_#{id}_ask_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force:) do
      result = get_l2_book(ticker)
      return result if result.failure?

      price = result.data.dig(:ask, :price)
      return Result::Failure.new("Hyperliquid has no asks for #{ticker.ticker}") if price.nil?
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

    start_time_ms = start_at.to_i * 1000
    end_time_ms = Time.now.utc.to_i * 1000

    result = client.candles_snapshot(
      coin: ticker.ticker,
      interval: interval,
      start_time: start_time_ms,
      end_time: end_time_ms
    )
    return result if result.failure?

    candles = result.data.map do |candle|
      [
        Time.at(candle['t'].to_i / 1000).utc,
        candle['o'].to_d,
        candle['h'].to_d,
        candle['l'].to_d,
        candle['c'].to_d,
        candle['v'].to_d
      ]
    end

    candles = build_candles_from_candles(candles:, timeframe:) if timeframe.in?([3.days, 1.week, 1.month])

    Result::Success.new(candles)
  end

  # Hyperliquid spot has no native market order type, so one is emulated with a limit priced THROUGH
  # the touch — the same trick Exchanges::Gemini uses. Crossing the book takes the resting liquidity
  # immediately instead of queueing behind it, which is the whole point of asking for a market order.
  #
  # These used to raise, because the only caller was the DCA leg and Bot::LimitOrderable forces
  # limit orders here anyway. Rebalancing is the caller that genuinely needs immediacy: its order
  # size comes from a snapshot of the portfolio, so an order that rests is an order sized against a
  # picture that has since changed.
  #
  # MARKET_CROSS is also a slippage bound, which a true market order does not have — worth having on
  # thin spot books.
  # ponytail: GTC, so an unfilled remainder rests rather than cancelling. Hyperliquid supports
  # immediate-or-cancel (`order_type: {limit: {tif: "Ioc"}}`), but our client wrapper does not expose
  # it — that is a honeymaker change. The remainder is handled: the rebalance state machine already
  # treats a resting order as pending and reconciles partial fills.
  MARKET_CROSS = BigDecimal('0.01')

  def market_buy(ticker:, amount:, amount_type:)
    set_crossing_order(ticker:, amount:, amount_type:, side: :buy)
  end

  def market_sell(ticker:, amount:, amount_type:)
    set_crossing_order(ticker:, amount:, amount_type:, side: :sell)
  end

  # Buy above the ask / sell below the bid, so an order crosses and fills against what is resting.
  # Overridden here rather than applied at placement time so that whoever SIZED the order used this
  # same number — see Exchange#market_price_for.
  def market_price_for(ticker:, side:)
    result = super
    return result if result.failure?

    multiplier = side == :buy ? (1 + MARKET_CROSS) : (1 - MARKET_CROSS)
    Result::Success.new(ticker.adjusted_price(price: result.data.to_d * multiplier))
  end

  # Hyperliquid rejects prices that aren't on tick size: at most 5 significant
  # figures and, for spot, at most (8 - szDecimals) decimal places. We translate
  # the significant-figure rule into a decimal-place count so floor/ceil/round
  # semantics are preserved (a buy limit set below market must never round up).
  # BigDecimal#exponent gives the power-of-ten magnitude exactly: 66.6 => 2,
  # 3.5 => 1, 0.0234 => -1. szDecimals is stored on the ticker as base_decimals.
  def adjusted_price(ticker:, price:, method: :floor)
    price = price.to_d
    return price if price.zero?

    max_decimals = SPOT_MAX_DECIMALS - ticker.base_decimals.to_i
    sig_decimals = SIGNIFICANT_FIGURES - price.exponent
    decimals     = sig_decimals.clamp(0, max_decimals) # >=10k clamps to 0 dp; integers are always valid
    price.public_send(method, decimals).to_d           # to_d: floor/ceil/round(0) returns an Integer
  end

  def get_order(order_id:)
    _coin, oid = parse_order_id(order_id)
    result = client.order_status(user: api_key&.key, oid: oid.to_i)

    if result.failure?
      # honeymaker now centralizes parsing; a not_found is the distinct unknownOid signal. Aged-out
      # filled orders are normal on Hyperliquid → recover the fill BEFORE declaring not-found (mirrors
      # Kraken). A non-not_found failure (transient/throttle) is propagated so the job retries.
      return result unless result.data.is_a?(Hash) && result.data[:not_found]

      return recover_order_from_fills(order_id)
    end

    Result::Success.new(build_order_data(order_id, result.data))
  end

  def get_orders(order_ids:)
    orders = {}
    missing = []
    order_ids.each do |order_id|
      result = get_order(order_id: order_id)
      if result.failure?
        # not_found (incl. unknownOid with no recoverable fill) → collect under :missing for the
        # open-orders sweep; anything else (transient/throttle) → abort the batch so the job retries.
        if result.data.is_a?(Hash) && result.data[:not_found]
          missing << order_id
          next
        end

        return result
      end

      orders[order_id] = result.data
    end

    Result::Success.new(orders: orders, missing: missing)
  end

  # userFills exhausts Hyperliquid's fill history, so a still-missing order is confirmed
  # never-executed → Bot::FetchAndUpdateOpenOrdersJob may stop wedging on young missing ids
  # (limit DCA self-heals via missed_quote_amount). Same contract as Kraken.
  def authoritative_missing_orders?
    true
  end

  def cancel_order(order_id:)
    return Result::Failure.new(ORDER_PLACEMENT_UNAVAILABLE_ERROR) unless order_placement_available?

    coin, oid = parse_order_id(order_id)
    result = client.cancel(coin: pair_of(order_id) || "#{coin}/USDC", oid: oid.to_i)
    return result if result.failure?

    Result::Success.new(order_id)
  end

  # The key is the account's address and the secret an API (agent) wallet's private key. Orders are
  # signed by the agent and act for the account that approved it, so the check has to tie the two
  # together: openOrders on the address is a public read and accepted any secret at all.
  # userRole (https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint) answers
  # {"role":"agent","data":{"user":<approver>}} for an approved agent, "missing" for one that never was or
  # has been pruned. The account's own private key is accepted as well (role "user"): it can trade too.
  # A sub-account address is refused: its agent's approver is the master, and orders signed here carry
  # no vaultAddress, so they would trade the master's balance.
  # A read that fails says nothing about the key and is handed back as it came.
  def get_api_key_validity(api_key:)
    return get_read_api_key_validity(api_key:) unless order_placement_available?

    signer = agent_address(api_key.secret)
    return Result::Success.new(false) if signer.nil?

    hm_client = Honeymaker.client('hyperliquid', api_key: api_key.key, api_secret: api_key.secret,
                                                 proxy: ExchangeProxy.for('hyperliquid'))
    # honeymaker 0.12.3 has no userRole; its own info request is used.
    result = hm_client.send(:post_info, { type: 'userRole', user: signer })
    return result if result.failure?

    role = result.data.is_a?(Hash) ? result.data : {}
    valid = case role['role']
            when 'agent' then role.dig('data', 'user').to_s.casecmp?(api_key.key)
            when 'user' then signer.casecmp?(api_key.key)
            else false
            end
    Result::Success.new(valid)
  end

  def minimum_amount_logic(**)
    # Hyperliquid is limit-only spot with a hard 10-USDC order floor and no per-asset base floor
    # (minimum_base_size stays 0). :base_and_quote_in_base converts minimum_quote_size (10) into a
    # per-order base minimum, ceil(10 / limit_price), compared against the FLOORED base size — so an
    # order whose value rounds below 10 (incl. dual-asset sub-orders) is skipped up front instead of
    # being sent and hard-rejected by the exchange.
    :base_and_quote_in_base
  end

  def get_ledger(api_key:, start_time: nil)
    hm_client = Honeymaker.client('hyperliquid', api_key: api_key.key, api_secret: api_key.secret,
                                                 proxy: ExchangeProxy.for('hyperliquid'))
    # A first sync reads from the beginning: userFills (no start) is only the 2000 most recent fills,
    # and the watermark then moved past everything older. A failed read fails the fetch, so the sync
    # never records a partial history as complete.
    start_ms = start_time ? (start_time.to_f * 1000).to_i : 0
    result = read_by_time(hm_client, :user_fills_by_time, user: api_key.key, start_ms:)
    return result if result.failure?

    # userFills returns the spot pair index (e.g. "@333"), not the token name — unlike
    # spot_balances, which returns "MU". Map it to the human base symbol via our ticker
    # table; fall back to the raw coin for an unknown/delisted pair.
    #
    # The same read returns the address's perp fills ("BTC", HIP-3 "xyz:TSLA"); a spot pair is named
    # "@<index>" or, for the oldest, "PURR/USDC". Pairs are quoted in USDH, USDT0 and USDE too.
    # The fee is in feeToken (the received token on a spot buy); a negative fee is a maker rebate,
    # not a fee.
    pairs = tickers.pluck(:ticker, :base, :quote).to_h { |ticker, base, quote| [ticker, [base, quote]] }
    spot_fills = result.data.select { |fill| fill['coin'].to_s.start_with?('@') || fill['coin'].to_s.include?('/') }
    entries = spot_fills.map do |fill|
      coin = fill['coin']
      base, quote = pairs[coin]
      is_buyer = fill['side'] == 'B'
      px = fill['px'].to_d
      sz = fill['sz'].to_d
      fee = fill['fee'].to_d
      { entry_type: is_buyer ? :buy : :sell,
        base_currency: base || coin, base_amount: sz,
        quote_currency: quote || 'USDC', quote_amount: (px * sz),
        fee_currency: fee.positive? ? fill['feeToken'].presence || quote || 'USDC' : nil,
        fee_amount: fee.positive? ? fee : nil,
        tx_id: fill['tid'].to_s, group_id: nil, description: nil,
        transacted_at: Time.at(fill['time'].to_i / 1000.0).utc, raw_data: fill }
    end

    updates = read_by_time(hm_client, :user_non_funding_ledger_updates, user: api_key.key, start_ms:)
    return updates if updates.failure?

    entries += updates.data.filter_map { |update| spot_transfer_entry(update, api_key.key) }
    Result::Success.new(entries)
  end

  private

  # https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint: the time-ranged user
  # reads answer oldest first from startTime (inclusive), at most 2000 rows a call.
  # ponytail: userFillsByTime serves only the 10000 most recent fills; anything older is gone at the venue.
  TIME_PAGE_LIMIT = 2000
  private_constant :TIME_PAGE_LIMIT

  # Every row from start_ms on. Each page starts at the newest time of the one before, so rows sharing
  # that millisecond are read again rather than skipped (busy accounts fill hundreds of times in one
  # ms). Identical rows are counted, not collapsed: a row is kept as often as the page that holds it
  # most. A full page that brings nothing new fails instead of looping.
  def read_by_time(hm_client, method, user:, start_ms:)
    rows = Hash.new(0)
    from = start_ms
    loop do
      result = hm_client.public_send(method, user:, start_time: from)
      return result if result.failure?

      page = Array(result.data)
      fresh = 0
      page.tally.each do |row, count|
        next if rows[row] >= count

        rows[row] = count
        fresh += 1
      end
      break if page.size < TIME_PAGE_LIMIT
      return Result::Failure.new("#{name} #{method} returned a full page with nothing new") if fresh.zero?

      from = page.map { |row| row['time'].to_i }.max
    end
    Result::Success.new(rows.flat_map { |row, count| [row] * count })
  end

  # One userNonFundingLedgerUpdates row as a deposit into or a withdrawal out of the spot balance — the
  # only balance this app reads. Shapes as the venue answers them (the docs show none):
  #   spotTransfer          {token, amount, user, destination, fee, feeToken}
  #   send                  {token, amount, user, destination, sourceDex, destinationDex}  ("spot" or "" = perp)
  #   accountClassTransfer  {usdc, toPerp}
  #   cStakingTransfer      {token, amount, isDeposit}  (into staking leaves spot)
  #   spotGenesis           {token, amount}
  # Bridge deposits and withdrawals, internalTransfer, vault and sub-account rows move the perp balance;
  # USDC reaches spot from there through an accountClassTransfer, which is what is recorded.
  # The hash is the transaction's, and staking transfers carry an all-zero one: those key on the event.
  def spot_transfer_entry(update, address)
    delta = update['delta'] || {}
    mine = ->(field) { delta[field].to_s.casecmp?(address) }
    token = delta['token']
    amount = delta['amount']
    type = case delta['type']
           when 'spotTransfer'
             mine.call('user') ? :withdrawal : :deposit
           when 'send'
             out = mine.call('user') && delta['sourceDex'] == 'spot'
             into = mine.call('destination') && delta['destinationDex'] == 'spot'
             if out && !into then :withdrawal
             elsif into && !out then :deposit
             end
           when 'accountClassTransfer'
             token = 'USDC'
             amount = delta['usdc']
             delta['toPerp'] ? :withdrawal : :deposit
           when 'cStakingTransfer'
             delta['isDeposit'] ? :withdrawal : :deposit
           when 'spotGenesis'
             :airdrop
           end
    return if type.nil?

    fee = type == :withdrawal ? delta['fee'].to_d : 0
    hash = update['hash'].to_s
    { entry_type: type, base_currency: token, base_amount: amount.to_d,
      quote_currency: nil, quote_amount: nil,
      fee_currency: fee.positive? ? delta['feeToken'].presence || 'USDC' : nil, fee_amount: fee.positive? ? fee : nil,
      tx_id: hash.match?(/\A0x0*\z/) ? nil : hash, group_id: nil, description: delta['type'],
      transacted_at: Time.at(update['time'].to_i / 1000.0).utc, raw_data: update }
  end

  # The pair an order was placed on. The id carries only the base, and a base trades against more
  # than one quote here (HYPE has USDC, USDH, USDT0 and USDE pairs), so a cancel aimed at "<base>/USDC"
  # missed every order on another quote. Nil for an order this app has no row for.
  def pair_of(order_id)
    order = transactions.find_by(external_id: order_id)
    order && tickers.find_by(base: order.base, quote: order.quote)&.ticker
  end

  # The address a private key signs as; nil for a secret that is not a key at all.
  def agent_address(secret)
    require 'eth'
    Eth::Key.new(priv: secret.to_s.delete_prefix('0x')).address.to_s
  rescue StandardError
    nil
  end

  def get_l2_book(ticker)
    cache_key = "exchange_#{id}_l2_book_#{ticker.id}"
    Rails.cache.fetch(cache_key, expires_in: 1.seconds) do
      result = client.l2_book(coin: ticker.ticker)
      return result if result.failure?

      # A side with no orders is an empty array: most RWA tokens have no book at all.
      bids, asks = result.data['levels']
      top = ->(level) { level && { price: level['px'].to_d, size: level['sz'].to_d } }
      Result::Success.new({ bid: top.call(Array(bids).first), ask: top.call(Array(asks).first) })
    end
  end

  # Order ID format: "COIN-oid" (e.g., "PURR-123456"). The oid is numeric, so it is what follows the
  # LAST dash: a coin with a dash in its name still splits right, and every stored id reads as before.
  def build_order_id(coin, oid)
    "#{coin}-#{oid}"
  end

  def parse_order_id(order_id)
    coin, _, oid = order_id.rpartition('-')
    [coin, oid]
  end

  def set_crossing_order(ticker:, amount:, amount_type:, side:)
    result = market_price_for(ticker: ticker, side: side)
    return result if result.failure?

    set_limit_order(ticker:, amount:, amount_type:, side:, price: result.data)
  end

  def set_limit_order(ticker:, amount:, amount_type:, side:, price:)
    return Result::Failure.new(ORDER_PLACEMENT_UNAVAILABLE_ERROR) unless order_placement_available?

    amount = ticker.adjusted_amount(amount:, amount_type:)
    price = ticker.adjusted_price(price:)
    return Result::Failure.new('Hyperliquid order failed: limit price must be positive') unless price.positive?

    # Hyperliquid's order API takes size in BASE units, so a quote amount must be
    # converted at the limit price (mirrors Exchanges::Alpaca#set_limit_order).
    size = amount_type == :quote ? (amount.to_d / price.to_d) : amount.to_d
    size = ticker.adjusted_amount(amount: size, amount_type: :base)

    result = client.order(
      coin: ticker.ticker,
      is_buy: side == :buy,
      size: size.to_d,
      limit_px: price.to_d
    )
    return result if result.failure?

    response = result.data
    return Result::Failure.new("Hyperliquid order failed: #{response}") unless response['status'] == 'ok'

    statuses = response.dig('response', 'data', 'statuses')
    # From here the venue said "ok": a missing acknowledgement is NOT a rejection, and the flag is
    # how a caller tells the two apart (Exchange#ambiguous_placement_error?).
    return Result::Failure.new('Hyperliquid order failed: no statuses returned', data: { unacknowledged: true }) if statuses.blank?

    order_status = statuses.first
    return Result::Failure.new("Hyperliquid order failed: #{order_status['error']}") if order_status['error'].present?

    # Order could be resting (limit) or filled immediately
    oid = order_status.dig('resting', 'oid') || order_status.dig('filled', 'oid')
    return Result::Failure.new('Hyperliquid order failed: no order ID returned', data: { unacknowledged: true }) if oid.nil?

    base_coin = ticker.base
    data = { order_id: build_order_id(base_coin, oid) }

    Result::Success.new(data)
  end

  # honeymaker returns order_type as :limit/:market; the Transaction enum is :limit_order/:market_order.
  ORDER_TYPE_MAP = { limit: :limit_order, market: :market_order }.freeze

  # Build the Transaction-shaped order_data from honeymaker's already-normalized fields, adding only
  # what's local to this app: the resolved ticker (by the raw universe coin, e.g. "@142"), the enum
  # order_type, empty error_messages, and the raw exchange_response. The PASSED order_id is preserved
  # (honeymaker's id is "<raw coin>-<oid>"; ours is "<base>-<oid>").
  def build_order_data(order_id, data)
    {
      order_id:,
      ticker: tickers.find_by(ticker: data[:coin]),
      price: data[:price],
      amount: data[:amount],
      quote_amount: data[:quote_amount],
      amount_exec: data[:amount_exec],
      quote_amount_exec: data[:quote_amount_exec],
      side: data[:side],
      order_type: ORDER_TYPE_MAP.fetch(data[:order_type], :limit_order),
      error_messages: [],
      status: data[:status],
      exchange_response: data[:raw]
    }
  end

  # unknownOid is normal for aged filled/canceled orders. Recover from userFillsByTime keyed on THIS
  # order's oid, bounding the lookback by the order's own row (mirrors Kraken's recover_missing_from_trades:
  # userFillsByTime needs a start_time, and get_order has none). Returns a synthesized :closed when
  # execution is proven, else a distinct not_found signal (so FetchAndUpdateOrderJob's StaleOrderResolver
  # path fires). A transient userFills failure is propagated, NOT degraded to not_found.
  def recover_order_from_fills(order_id)
    _coin, oid = parse_order_id(order_id)
    since = transactions.where(external_id: order_id).minimum(:created_at)
    return not_found_failure(order_id) unless since

    start_ms = ((since - 1.hour).to_f * 1000).to_i # ms, buffered for immediate-fill / clock skew
    # Every page: one call stops at 2000 fills, and on a busy account the order's own fills can sit past
    # them, which would read as "never executed" to a caller that trusts the miss.
    result = read_by_time(client, :user_fills_by_time, user: api_key&.key, start_ms:)
    return result if result.failure?

    fills = result.data.select { |f| f['oid'].to_s == oid.to_s }
    return not_found_failure(order_id) if fills.empty?

    Result::Success.new(order_data_from_fills(order_id, fills))
  end

  def not_found_failure(order_id)
    Result::Failure.new("Hyperliquid did not return data for order #{order_id}",
                        data: { not_found: true, missing_ids: [order_id] })
  end

  def order_data_from_fills(order_id, fills)
    amount_exec = fills.sum(BigDecimal('0')) { |f| f['sz'].to_d }
    quote_amount_exec = fills.sum(BigDecimal('0')) { |f| f['px'].to_d * f['sz'].to_d }
    {
      order_id:,
      ticker: tickers.find_by(ticker: fills.first['coin']),
      price: amount_exec.positive? ? (quote_amount_exec / amount_exec) : nil,
      amount: nil,
      quote_amount: nil,
      amount_exec:,
      quote_amount_exec:,
      side: fills.first['side'] == 'B' ? :buy : :sell,
      order_type: :limit_order,
      error_messages: [],
      status: :closed,
      exchange_response: { 'fills' => fills }
    }
  end

  # Suffix-aware so the whole Hyperliquid cancel family maps correctly; a triggered order has fired
  # and become a live resting order → :open. Production parsing now lives in honeymaker; this mirror is
  # retained for the parse_order_status characterization suite. An unmapped status is logged, not raised.
  def parse_order_status(status)
    case status
    when 'filled'
      :closed
    when 'open', 'triggered'
      :open
    else
      str = status.to_s
      if str.match?(/cancel/i) || str.match?(/reject/i)
        :cancelled
      else
        Rails.logger.warn("[Hyperliquid] Unmapped order status: #{status.inspect}")
        :unknown
      end
    end
  end
end
