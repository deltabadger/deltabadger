class Exchanges::Alpaca < Exchange
  ERRORS = {
    insufficient_funds: ['insufficient buying power'],
    # Alpaca's OWN word for a rejected key, from the trading host's JSON body
    # {"message":"unauthorized."} — this is the string production recorded in ApiKey#last_sync_error
    # while the key still read `correct`, because the list used to be empty and
    # Exchange#invalid_key_error? returns false on an empty one. Matched as a substring, so the
    # trailing period is not load-bearing.
    #
    # Clients::Alpaca's "HTTP 401" fallback (its HTML-body case, which is how the same rejection
    # arrives from the market-data host) is deliberately NOT listed: that string is synthesised from
    # the status by our own client, so a WAF or edge 401 would produce it just as readily, and
    # condemning on it would be the bare-status rule wearing a costume. It still SURFACES through
    # Exchange#invalid_key_error?(status:) — the bot error stays truthful either way; only the
    # persistent :incorrect flip waits for Alpaca to say it.
    invalid_key: ['unauthorized']
  }.freeze

  # Alpaca's tradable crypto universe (30+ assets as of 2026-07, growing) mapped to the
  # canonical CoinGecko id so synced tickers resolve to the SAME Asset already used by
  # Kraken/Binance/etc, instead of minting a duplicate identity the way stocks do. Stocks
  # have no CoinGecko presence to collide with (alpaca_<uuid> is safe there) — crypto does,
  # so this table exists to avoid a second, disconnected "AAVE" appearing in the
  # tracker/portfolio.
  # Hand-maintained, not derived: this is a SNAPSHOT — verify against a live
  # GET /v2/assets?asset_class=crypto response before each production sync, not just when
  # Alpaca visibly adds a pair. Keyed by the BASE symbol only (Alpaca's `symbol` field is the
  # full pair, e.g. "AAVE/USD" — parsed before this lookup).
  CRYPTO_COINGECKO_IDS = {
    'AAVE' => 'aave',
    'ADA' => 'cardano',
    'ARB' => 'arbitrum',
    'AVAX' => 'avalanche-2',
    'BAT' => 'basic-attention-token',
    'BCH' => 'bitcoin-cash',
    'BONK' => 'bonk',
    'BTC' => 'bitcoin',
    'CRV' => 'curve-dao-token',
    'DOGE' => 'dogecoin',
    'DOT' => 'polkadot',
    'ETH' => 'ethereum',
    'FIL' => 'filecoin',
    'GRT' => 'the-graph',
    'HYPE' => 'hyperliquid',
    'LDO' => 'lido-dao',
    'LINK' => 'chainlink',
    'LTC' => 'litecoin',
    'ONDO' => 'ondo-finance',
    'PAXG' => 'pax-gold',
    'PEPE' => 'pepe',
    'POL' => 'polygon-ecosystem-token',
    'RENDER' => 'render-token',
    'SHIB' => 'shiba-inu',
    'SKY' => 'sky',
    'SOL' => 'solana',
    'SUSHI' => 'sushi',
    'TRUMP' => 'official-trump',
    'UNI' => 'uniswap',
    'USDC' => 'usd-coin',
    'USDG' => 'global-dollar',
    'USDT' => 'tether',
    'WIF' => 'dogwifcoin',
    'XRP' => 'ripple',
    'XTZ' => 'tezos',
    'YFI' => 'yearn-finance'
  }.freeze

  include Exchange::Dryable

  attr_reader :api_key

  def coingecko_id
    nil
  end

  def known_errors
    ERRORS
  end

  def set_client(api_key: nil)
    @api_key = api_key
    @client = Clients::Alpaca.new(
      api_key: api_key&.key,
      api_secret: api_key&.secret,
      paper: paper_mode?(api_key)
    )
  end

  def requires_passphrase?
    true
  end

  def supports_withdrawal?
    false
  end

  def minimum_amount_logic(**)
    :quote
  end

  def market_open?(tickers: nil)
    return true if all_crypto?(tickers)

    clock = get_clock_cached
    return true if clock.nil?

    clock['is_open'] == true
  end

  # Margin accounts run settled cash at or below zero by design; buying power is what Alpaca
  # checks when an order lands, so cash alone false-alarms every margin user.
  #
  # ponytail: all_crypto? is approximate — non-marginable equities aren't flagged in our catalog,
  # DcaIndex#tickers is every quote-matching ticker rather than the bot's allocations, and a mixed
  # dual bot has both. All three then resolve to :buying_power, i.e. a missed warning, never the
  # false alarm this fixes. any_crypto? would invert that and keep false-alarming index bots.
  # Upgrade path: persist Alpaca's `marginable` flag on Ticker/ExchangeAsset (it's venue metadata)
  # and give bots a funding_tickers that reports real allocations.
  def spendable_balance(balance, tickers: nil)
    # Crypto is non-marginable at Alpaca — it spends settled cash, not stock-backed leverage.
    key = all_crypto?(tickers) ? :non_marginable_buying_power : :buying_power
    balance[key] || balance[:free]
  end

  def next_market_open_at(tickers: nil)
    return Time.current if all_crypto?(tickers)

    clock = get_clock_cached
    return Time.current if clock.nil?

    Time.parse(clock['next_open'])
  end

  def get_tickers_info(force: false)
    cache_key = "exchange_#{id}_tickers_info"
    tickers_info = Rails.cache.fetch(cache_key, expires_in: 1.hour, force: force) do
      result = client.get_assets(status: 'active', asset_class: 'us_equity')
      return Result::Failure.new("Failed to get #{name} assets") if result.failure?

      result.data.select { |a| a['tradable'] && a['fractionable'] }.map do |asset|
        {
          ticker: asset['symbol'],
          base: asset['symbol'],
          quote: 'USD',
          minimum_base_size: 0.000000001.to_d, # fractional shares
          minimum_quote_size: 1.to_d, # $1 minimum
          maximum_base_size: 100_000.to_d,
          maximum_quote_size: 10_000_000.to_d,
          base_decimals: 9,
          quote_decimals: 2,
          price_decimals: 2,
          available: true,
          trading_enabled: true
        }
      end
    end

    Result::Success.new(tickers_info)
  end

  # Alpaca quotes stock prices directly in USD; the central MarketData feed
  # (crypto-only) has no entries for them. Only map stock assets here — USD
  # cash (category 'Fiat') is still priced via MarketData (always 1.0).
  def get_usd_prices(assets:)
    stock_assets = Array(assets).select { |a| a.category == 'Stock' }
    return Result::Success.new({}) if stock_assets.empty?

    result = get_tickers_prices(symbols: stock_assets.map(&:symbol).uniq)
    return result if result.failure?

    prices_by_symbol = result.data
    mapped = stock_assets.each_with_object({}) do |asset, h|
      price = prices_by_symbol[asset.symbol]
      h[asset.external_id] = price if price
    end
    Result::Success.new(mapped)
  end

  def get_tickers_prices(force: false, symbols: nil)
    symbols ||= tickers.available.pluck(:ticker)
    return Result::Success.new({}) if symbols.empty?

    stock_symbols, crypto_symbols = symbols.partition { |s| !s.include?('/') }
    prices = {}

    if stock_symbols.any?
      result = get_stock_tickers_prices(stock_symbols, force: force)
      return result if result.failure?

      prices.merge!(result.data)
    end

    if crypto_symbols.any?
      result = get_crypto_tickers_prices(crypto_symbols, force: force)
      return result if result.failure?

      prices.merge!(result.data)
    end

    Result::Success.new(prices)
  end

  def get_stock_tickers_prices(symbols, force:)
    sorted = symbols.sort
    cache_key = "exchange_#{id}_prices_#{Digest::MD5.hexdigest(sorted.join(','))}"
    tickers_prices = Rails.cache.fetch(cache_key, expires_in: 1.minute, force: force) do
      result = market_data_client.get_snapshots(symbols: sorted)
      return result if result.failure?

      result.data.transform_values { |snapshot| snapshot.dig('latestTrade', 'p').to_d }
    end

    Result::Success.new(tickers_prices)
  end

  def get_crypto_tickers_prices(symbols, force:)
    sorted = symbols.sort
    cache_key = "exchange_#{id}_crypto_prices_#{Digest::MD5.hexdigest(sorted.join(','))}"
    tickers_prices = Rails.cache.fetch(cache_key, expires_in: 1.minute, force: force) do
      result = market_data_client.get_crypto_latest_trade(symbols: sorted)
      return result if result.failure?

      result.data.fetch('trades', {}).transform_values { |trade| trade['p'].to_d }
    end

    Result::Success.new(tickers_prices)
  end

  def get_balances(asset_ids: nil)
    # Get cash balance from account
    account_result = client.get_account
    return account_result if account_result.failure?

    # Get stock positions
    positions_result = client.get_positions
    return positions_result if positions_result.failure?

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.to_h do |asset_id|
      [asset_id, { free: 0, locked: 0 }]
    end

    # USD cash. :free stays settled cash — AccountBalance::Sync values the portfolio from it, and
    # buying power is borrowed, not owned. The spend figures ride along for #spendable_balance.
    # &.to_d, not .to_d: nil.to_d is 0, which would turn an absent field into a hard zero and make
    # every account look broke.
    usd_asset = asset_from_symbol('USD')
    if usd_asset && asset_ids.include?(usd_asset.id)
      account = account_result.data
      balances[usd_asset.id] = {
        free: account['cash'].to_d,
        locked: 0,
        buying_power: account['buying_power']&.to_d,
        non_marginable_buying_power: account['non_marginable_buying_power']&.to_d
      }
    end

    # Positions. Stocks resolve via the existing bare-symbol map (position symbol ==
    # ticker.base, e.g. "AAPL"). Crypto positions come back compact-concatenated (e.g.
    # "AAVEUSD") — a THIRD format, distinct from both that bare form and the "AAVE/USD" pair
    # format orders/quotes use — so they need their own lookup.
    positions_result.data.each do |position|
      asset = asset_from_symbol(position['symbol']) || asset_from_crypto_position_symbol(position['symbol'])
      next unless asset && asset_ids.include?(asset.id)

      qty = position['qty'].to_d
      balances[asset.id] = { free: qty, locked: 0 }
    end

    Result::Success.new(balances)
  end

  def get_last_price(ticker:, force: false)
    cache_key = "exchange_#{id}_last_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      price = if crypto_ticker?(ticker)
                result = market_data_client.get_crypto_latest_trade(symbols: [ticker.ticker])
                return result if result.failure?

                result.data.dig('trades', ticker.ticker, 'p').to_d
              else
                result = market_data_client.get_latest_trade(symbol: ticker.base)
                return result if result.failure?

                result.data.dig('trade', 'p').to_d
              end
      raise "Wrong last price for #{ticker.base}: #{price}" if price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_bid_price(ticker:, force: false)
    cache_key = "exchange_#{id}_bid_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      price = if crypto_ticker?(ticker)
                result = market_data_client.get_crypto_latest_quote(symbols: [ticker.ticker])
                return result if result.failure?

                result.data.dig('quotes', ticker.ticker, 'bp').to_d
              else
                result = market_data_client.get_latest_quote(symbol: ticker.base)
                return result if result.failure?

                result.data.dig('quote', 'bp').to_d
              end
      raise "Wrong bid price for #{ticker.base}: #{price}" if price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_ask_price(ticker:, force: false)
    cache_key = "exchange_#{id}_ask_price_#{ticker.id}"
    price = Rails.cache.fetch(cache_key, expires_in: 5.seconds, force: force) do
      price = if crypto_ticker?(ticker)
                result = market_data_client.get_crypto_latest_quote(symbols: [ticker.ticker])
                return result if result.failure?

                result.data.dig('quotes', ticker.ticker, 'ap').to_d
              else
                result = market_data_client.get_latest_quote(symbol: ticker.base)
                return result if result.failure?

                result.data.dig('quote', 'ap').to_d
              end
      raise "Wrong ask price for #{ticker.base}: #{price}" if price.zero?

      price
    end

    Result::Success.new(price)
  end

  def get_indicator_candles(ticker:, start_at:, timeframe:)
    get_candles(ticker: ticker, start_at: start_at, timeframe: timeframe, adjustment: 'split')
  end

  # A stock's history is restated by every split and the whole series moves; a crypto pair on the
  # same venue has no corporate actions at all.
  def restated_candles?(ticker)
    !crypto_ticker?(ticker)
  end

  # `adjustment` is Alpaca's corporate-action basis and is deliberately nil by default — see
  # Exchange#get_indicator_candles for which callers want which. It reaches the stock branch only:
  # the crypto bars endpoint has no such parameter, and needs none.
  def get_candles(ticker:, start_at:, timeframe:, adjustment: nil)
    alpaca_timeframes = {
      1.minute => '1Min',
      5.minutes => '5Min',
      15.minutes => '15Min',
      30.minutes => '30Min',
      1.hour => '1Hour',
      4.hours => '4Hour',
      1.day => '1Day',
      1.week => '1Week',
      1.month => '1Month'
    }
    tf = alpaca_timeframes[timeframe] || '1Day'
    is_crypto = crypto_ticker?(ticker)

    result = if is_crypto
               market_data_client.get_crypto_bars(symbol: ticker.ticker, timeframe: tf, start_time: start_at.iso8601)
             else
               market_data_client.get_bars(symbol: ticker.base, timeframe: tf,
                                           start_time: start_at.iso8601, adjustment: adjustment)
             end
    return result if result.failure?

    bars = is_crypto ? (result.data.dig('bars', ticker.ticker) || []) : (result.data['bars'] || [])
    candles = bars.map do |bar|
      [
        Time.parse(bar['t']).utc,
        bar['o'].to_d,
        bar['h'].to_d,
        bar['l'].to_d,
        bar['c'].to_d,
        bar['v'].to_d
      ]
    end

    Result::Success.new(candles)
  end

  def market_buy(ticker:, amount:, amount_type:)
    set_market_order(ticker: ticker, amount: amount, amount_type: amount_type, side: :buy)
  end

  def market_sell(ticker:, amount:, amount_type:)
    set_market_order(ticker: ticker, amount: amount, amount_type: amount_type, side: :sell)
  end

  def limit_buy(ticker:, amount:, amount_type:, price:)
    set_limit_order(ticker: ticker, amount: amount, amount_type: amount_type, side: :buy, price: price)
  end

  def limit_sell(ticker:, amount:, amount_type:, price:)
    set_limit_order(ticker: ticker, amount: amount, amount_type: amount_type, side: :sell, price: price)
  end

  def get_order(order_id:)
    result = client.get_order(order_id: order_id)
    return result if result.failure?

    normalized = parse_order_data(result.data)
    Result::Success.new(normalized)
  end

  def get_orders(order_ids:)
    orders = {}
    order_ids.each do |order_id|
      result = client.get_order(order_id: order_id)
      return result if result.failure?

      orders[order_id] = parse_order_data(result.data)
    end

    Result::Success.new(orders: orders, missing: [])
  end

  def list_open_orders
    result = client.list_orders(status: 'open')
    return result if result.failure?

    orders = result.data.map { |order| parse_order_data(order) }
    Result::Success.new(orders)
  end

  def cancel_order(order_id:)
    result = client.cancel_order(order_id: order_id)
    return result if result.failure?

    Result::Success.new(order_id)
  end

  def get_api_key_validity(api_key:)
    result = Clients::Alpaca.new(
      api_key: api_key.key,
      api_secret: api_key.secret,
      paper: paper_mode?(api_key)
    ).get_account

    if result.success?
      # Paper accounts return 'ACTIVE', live accounts also return 'ACTIVE'
      Result::Success.new(result.data['status'] == 'ACTIVE')
    elsif result.data.is_a?(Hash) && result.data[:status] == 401
      Result::Success.new(false)
    else
      result
    end
  end

  def fetch_withdrawal_fees!
    Result::Success.new({})
  end

  # Search tradable stocks from Alpaca API
  # @param query [String] search term
  # @return [Array<Hash>] matching stocks
  def get_ledger(api_key:, start_time: nil)
    set_client(api_key: api_key)

    entries = []
    page_token = nil
    loop do
      params = { direction: 'asc', page_size: 100, page_token: page_token }.compact
      params[:after] = start_time.iso8601 if start_time
      result = client.get_account_activities(**params)
      return result if result.failure?

      activities = Array(result.data)
      break if activities.empty?

      # page_token is an EXCLUSIVE cursor on id, so the last id of a page is always past the
      # token that fetched it. If it isn't, the cursor was ignored and we'd re-fetch the same
      # page forever — stop instead of hanging the sync on an unbounded loop.
      next_token = activities.last['id']
      if next_token == page_token
        Rails.logger.warn("[#{name_id}] Alpaca ledger pagination stalled at page token #{page_token}")
        break
      end

      activities.each do |activity|
        entry = normalize_activity(activity)
        entries << entry if entry
      end
      page_token = next_token
      break if activities.size < 100
    end

    Result::Success.new(merge_split_entries(entries))
  end

  def search_assets(query)
    all_assets = get_cached_assets
    return [] if all_assets.blank? || query.blank?

    query_down = query.downcase
    all_assets.select do |a|
      a['symbol'].downcase.include?(query_down) ||
        a['name']&.downcase&.include?(query_down)
    end.first(20)
  end

  # "10:1" — the factor a restatement applied, from the position before it and the position after.
  #
  # Rationalized, not divided: fractional shares turn a clean 10x into 9.999...:1, and a
  # three-for-two reads as 1.5:1 rather than the ratio a person recognises.
  #
  # The tolerance is RELATIVE. A fixed one is a fraction of the factor at 10:1 and larger than it
  # at 1:100, so an absolute 0.001 rounded an exact 1-for-50 reverse split to 1:48 and an exact
  # 1-for-100 to 1:91 — it accepts any simpler fraction within the window, and every small
  # denominator sits inside one that wide. A per-mille of the factor itself holds at both ends.
  #
  # nil unless the two counts actually describe a restatement — both present, both positive, and
  # different from each other. A pair that nets to nothing, one that only ever added shares, and
  # one that took five away and put five back are all something other than a split; "1:1" would
  # be a ratio for an event that changed no share count.
  def self.split_ratio_label(old_count, new_count)
    old_count = old_count.to_d
    new_count = new_count.to_d
    return nil unless old_count.positive? && new_count.positive? && old_count != new_count

    factor = (new_count / old_count).to_f
    rational = factor.rationalize(factor * 0.001)
    # Counts closer together than the tolerance rationalize to 1 — 1000 shares becoming 1001 is
    # not a split, and "1:1" would be a ratio asserting nothing happened. Say nothing instead.
    return nil if rational == 1

    "#{rational.numerator}:#{rational.denominator}"
  end

  private

  # Market data (data.alpaca.markets) requires auth but is read-only and host-separate
  # from trading. Build a throwaway client so these reads never mutate @client/@api_key —
  # account ops depend on those being the caller-set key. Prefer an explicitly-set key
  # (execution path already set one); otherwise resolve a valid trading key for this
  # exchange (the dashboard/metrics path, where no key was set).
  def market_data_client
    key = api_key || market_data_api_key
    Clients::Alpaca.new(api_key: key&.key, api_secret: key&.secret, paper: paper_mode?(key))
  end

  def crypto_ticker?(ticker)
    ticker&.base_asset&.category == 'Cryptocurrency'
  end

  def all_crypto?(tickers)
    list = Array(tickers)
    list.present? && list.all? { |t| crypto_ticker?(t) }
  end

  def asset_from_crypto_position_symbol(symbol)
    crypto_position_index[symbol]&.base_asset
  end

  def crypto_position_index
    @crypto_position_index ||= tickers.available.includes(:base_asset)
                                      .select { |t| crypto_ticker?(t) }
                                      .index_by { |t| "#{t.base}#{t.quote}" }
  end

  # Any valid trading key authenticates Alpaca market data (account-agnostic reads).
  def market_data_api_key
    api_keys.correct.find_by(key_type: :trading) || api_keys.find_by(key_type: :trading)
  end

  # Default to paper mode when passphrase is nil (safe default for testing)
  def paper_mode?(api_key)
    return true if api_key.nil?

    api_key.passphrase != 'live'
  end

  # nil means "could not ask", which #market_open? deliberately reads as open — a clock blip must not
  # pause everyone's trading. A rejected key is NOT that: it fails every tick forever, so fail-open
  # turned a dead key into "the market is open" at 04:00 UTC and sent the bot on to work that could
  # only fail. Raise it here, at the first call the job makes, instead of somewhere downstream that
  # cannot tell why nothing has a price. Nothing is cached on either path.
  def get_clock_cached
    Rails.cache.fetch("exchange_#{id}_clock", expires_in: 1.minute) do
      result = client.get_clock
      if result.failure?
        raise_on_invalid_key!(result)
        return nil
      end

      result.data
    end
  end

  def get_cached_assets
    Rails.cache.fetch("exchange_#{id}_tradable_assets", expires_in: 1.hour) do
      result = client.get_assets(status: 'active', asset_class: 'us_equity')
      return [] if result.failure?

      result.data.select { |a| a['tradable'] && a['fractionable'] }
    end
  end

  DIVIDEND_INCOME_TYPES = %w[DIV DIVCGL DIVCGS CGD DIVTXEX].freeze
  WITHHOLDING_TYPES = %w[DIVNRA DIVFT DIVTW INTNRA INTTW].freeze
  CASH_FEE_TYPES = %w[FEE DIVFEE PTC].freeze
  CASH_JOURNAL_TYPES = %w[JNLC OCT ACATC].freeze
  SPLIT_TYPES = %w[SPLIT SSP].freeze

  def normalize_activity(activity)
    type = activity['activity_type']
    return normalize_fill(activity) if type == 'FILL'
    return nil if activity['status'] == 'canceled'

    case type
    when 'CSD'
      normalize_cash_transfer(activity, :deposit)
    when 'CSW'
      normalize_cash_transfer(activity, :withdrawal)
    when *CASH_JOURNAL_TYPES
      entry_type = activity['net_amount'].to_d.negative? ? :withdrawal : :deposit
      normalize_cash_transfer(activity, entry_type)
    when *DIVIDEND_INCOME_TYPES
      normalize_non_trade(
        activity,
        :other_income,
        "Dividend (#{activity['symbol']})",
        quote_currency: activity['symbol']
      )
    when *WITHHOLDING_TYPES
      symbol = activity['symbol']
      normalize_non_trade(
        activity,
        :withholding_tax,
        "Withholding (#{symbol || 'interest'})",
        quote_currency: symbol
      )
    when 'DIVROC'
      normalize_return_of_capital(activity)
    when *CASH_FEE_TYPES
      normalize_non_trade(activity, :fee, nil)
    when 'CFEE'
      normalize_crypto_fee(activity)
    when 'INT', 'PTR'
      normalize_non_trade(activity, :other_income, nil)
    when *SPLIT_TYPES
      normalize_split(activity)
    else
      # Multi-leg mergers, spinoffs, and option events must never half-mutate share counts.
      # Keep them and future activity types inert and verbatim so reports can flag incomplete symbols.
      normalize_unsupported_activity(activity)
    end
  end

  def normalize_fill(activity)
    qty = activity['qty'].to_d
    price = activity['price'].to_d
    symbol = activity['symbol']
    base_currency, quote_currency = if symbol&.include?('/')
                                      symbol.split('/', 2)
                                    elsif (ticker = crypto_position_index[symbol])
                                      [ticker.base, ticker.quote]
                                    else
                                      [symbol, 'USD']
                                    end

    {
      entry_type: activity['side'] == 'buy' ? :buy : :sell,
      base_currency: base_currency,
      base_amount: qty,
      quote_currency: quote_currency,
      quote_amount: qty * price,
      fee_currency: nil,
      fee_amount: nil,
      tx_id: activity['id'],
      group_id: nil,
      description: nil,
      transacted_at: Time.parse(activity['transaction_time']).utc,
      raw_data: activity
    }
  end

  def normalize_cash_transfer(activity, entry_type)
    {
      entry_type: entry_type,
      base_currency: 'USD',
      base_amount: activity['net_amount'].to_d.abs,
      quote_currency: nil,
      quote_amount: nil,
      fee_currency: nil,
      fee_amount: nil,
      tx_id: activity['id'],
      group_id: activity['group_id'],
      description: nil,
      transacted_at: non_trade_timestamp(activity),
      raw_data: activity
    }
  end

  # `.abs` throws away the sign, which matters for a withholding debit — Alpaca books it negative.
  # It is deliberate and compensated: `Tax::BrokerReport#handle_withholding` reads the magnitude
  # (`record.base_amount.to_d.abs`) and adds it to the Zeile 41 CREDIT, so the sign would have to be
  # re-flipped there anyway. Change one and you must change the other.
  def normalize_non_trade(activity, entry_type, description, quote_currency: nil)
    {
      entry_type: entry_type,
      base_currency: 'USD',
      base_amount: activity['net_amount'].to_d.abs,
      quote_currency: quote_currency,
      quote_amount: nil,
      fee_currency: nil,
      fee_amount: nil,
      tx_id: activity['id'],
      group_id: activity['group_id'],
      description: description,
      transacted_at: non_trade_timestamp(activity),
      raw_data: activity
    }
  end

  def normalize_return_of_capital(activity)
    {
      entry_type: :return_of_capital,
      base_currency: activity['symbol'],
      base_amount: activity['qty'].to_d,
      quote_currency: 'USD',
      quote_amount: activity['net_amount'].to_d,
      fee_currency: nil,
      fee_amount: nil,
      tx_id: activity['id'],
      group_id: activity['group_id'],
      description: "Return of capital (#{activity['symbol']})",
      transacted_at: non_trade_timestamp(activity),
      raw_data: activity
    }
  end

  def normalize_split(activity)
    {
      entry_type: :adjustment,
      base_currency: activity['symbol'],
      base_amount: activity['qty'].to_d,
      quote_currency: nil,
      quote_amount: nil,
      fee_currency: nil,
      fee_amount: nil,
      tx_id: activity['id'],
      group_id: activity['group_id'],
      description: "Split (#{activity['symbol']})",
      transacted_at: non_trade_timestamp(activity),
      # The marker, not the entry type, is what says a split. `adjustment` is generic — a future
      # venue correction lands in it, and a CSV import can create one with no provenance at all —
      # so anything that reacts to a split keys off this.
      raw_data: activity.merge('corporate_action' => 'split')
    }
  end

  def normalize_unsupported_activity(activity)
    {
      entry_type: :unsupported_activity,
      base_currency: activity['symbol'] || 'USD',
      base_amount: activity['qty'].to_d,
      quote_currency: nil,
      quote_amount: activity['net_amount']&.to_d,
      fee_currency: nil,
      fee_amount: nil,
      tx_id: activity['id'],
      group_id: activity['group_id'],
      description: activity['activity_type'],
      transacted_at: non_trade_timestamp(activity),
      raw_data: activity
    }
  end

  def non_trade_timestamp(activity)
    timestamp = activity['date'] || activity['transaction_time']
    timestamp && Time.zone.parse(timestamp).utc
  end

  def merge_split_entries(entries)
    # Alpaca ships splits as remove-old-qty + add-new-qty pairs; engines need one signed net delta.
    entries.chunk_while do |previous, current|
      SPLIT_TYPES.include?(previous.dig(:raw_data, 'activity_type')) &&
        SPLIT_TYPES.include?(current.dig(:raw_data, 'activity_type')) &&
        previous[:base_currency] == current[:base_currency] &&
        previous.dig(:raw_data, 'date') == current.dig(:raw_data, 'date')
    end.map do |group|
      next group.first if group.one?

      first = group.first
      ratio = split_ratio(group)
      first.merge(
        base_amount: group.sum { |entry| entry[:base_amount] },
        description: [first[:description], ratio].compact.join(' '),
        raw_data: first[:raw_data].merge(
          { 'merged_activity_ids' => group.map { |entry| entry[:raw_data]['id'] },
            'split_ratio' => ratio }.compact
        )
      )
    end
  end

  # The counts the legs already carry: the removal is the old position, the addition the new one.
  # Summed rather than paired, so a venue that ships the same event in more than two legs still
  # reduces to one factor.
  def split_ratio(group)
    amounts = group.map { |entry| entry[:base_amount].to_d }
    self.class.split_ratio_label(-amounts.select(&:negative?).sum, amounts.select(&:positive?).sum)
  end

  # CFEE (Alpaca's crypto trading fee) is NOT USD-denominated like the stock-side FEE — its
  # net_amount is always "0" (see Alpaca's own example: description "Coin Pair Transaction Fee
  # (Non USD)"). The real fee is `qty` (negative), charged in the base crypto asset itself, and
  # `symbol` is the COMPACT pair format ("ETHUSD", no slash) — resolved the same way balances are
  # (asset_from_crypto_position_symbol), not the slash-separated FILL format.
  def normalize_crypto_fee(activity)
    asset = asset_from_crypto_position_symbol(activity['symbol'])

    {
      entry_type: :fee,
      base_currency: asset&.symbol || activity['symbol'],
      base_amount: activity['qty'].to_d.abs,
      quote_currency: nil,
      quote_amount: nil,
      fee_currency: nil,
      fee_amount: nil,
      tx_id: activity['id'],
      group_id: activity['group_id'],
      description: nil,
      transacted_at: non_trade_timestamp(activity),
      raw_data: activity
    }
  end

  def set_market_order(ticker:, amount:, amount_type:, side:)
    amount = ticker.adjusted_amount(amount: amount, amount_type: amount_type)
    tif = crypto_ticker?(ticker) ? 'gtc' : 'day'

    result = if amount_type == :quote
               # Use notional (dollar amount) for market orders
               client.create_order(
                 symbol: ticker.ticker,
                 side: side.to_s,
                 type: 'market',
                 time_in_force: tif,
                 notional: format("%.#{ticker.quote_decimals}f", amount.to_d)
               )
             else
               client.create_order(
                 symbol: ticker.ticker,
                 side: side.to_s,
                 type: 'market',
                 time_in_force: tif,
                 qty: format("%.#{ticker.base_decimals}f", amount.to_d)
               )
             end
    return result if result.failure?

    data = { order_id: result.data['id'] }
    Result::Success.new(data)
  end

  def set_limit_order(ticker:, amount:, amount_type:, side:, price:)
    amount = ticker.adjusted_amount(amount: amount, amount_type: amount_type)

    # Limit orders require qty (shares), not notional
    qty = if amount_type == :quote
            (amount.to_d / price.to_d)
          else
            amount.to_d
          end
    qty = ticker.adjusted_amount(amount: qty, amount_type: :base)

    price = ticker.adjusted_price(price: price)

    result = client.create_order(
      symbol: ticker.ticker,
      side: side.to_s,
      type: 'limit',
      time_in_force: crypto_ticker?(ticker) ? 'gtc' : 'day',
      qty: format("%.#{ticker.base_decimals}f", qty.to_d),
      limit_price: format("%.#{ticker.price_decimals}f", price.to_d)
    )
    return result if result.failure?

    data = { order_id: result.data['id'] }
    Result::Success.new(data)
  end

  def parse_order_data(order_data)
    ticker_record = tickers.find_by(ticker: order_data['symbol'])
    order_type = order_data['type'] == 'limit' ? :limit_order : :market_order
    side = order_data['side']&.to_sym
    filled_qty = order_data['filled_qty'].to_d
    filled_avg_price = order_data['filled_avg_price']&.to_d
    notional = order_data['notional']&.to_d
    qty = order_data['qty']&.to_d
    limit_price = order_data['limit_price']&.to_d
    price = filled_avg_price.present? && filled_avg_price.positive? ? filled_avg_price : (limit_price || 0)

    {
      order_id: order_data['id'],
      ticker: ticker_record,
      price: price,
      amount: qty,
      quote_amount: notional,
      amount_exec: filled_qty,
      quote_amount_exec: filled_qty * (filled_avg_price || 0),
      side: side,
      order_type: order_type,
      error_messages: [],
      status: parse_order_status(order_data['status']),
      exchange_response: order_data
    }
  end

  def parse_order_status(status)
    case status
    when 'new', 'accepted', 'pending_new'
      :open
    when 'filled'
      :closed
    when 'canceled', 'expired', 'replaced'
      :cancelled
    when 'rejected'
      :failed
    else
      :unknown
    end
  end
end
