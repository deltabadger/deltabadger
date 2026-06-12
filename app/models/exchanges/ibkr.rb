class Exchanges::Ibkr < Exchange
  # IBKR error buckets. NOTE: :invalid_key is intentionally EMPTY for now — a session/competing-
  # login/expired-LST blip must NEVER flip a valid key to :incorrect (ApiKeyFailureHandling). We
  # only add genuine credential-rejection strings here once observed in the staging logs.
  ERRORS = {
    insufficient_funds: ['insufficient', 'buying power'],
    invalid_key: [],
    transient: ['not authenticated', 'competing', 'session', 'Please query /accounts first',
                'live session token', 'Bad Request: no bridge']
  }.freeze

  # IBKR market-data snapshot field codes.
  FIELD_LAST = '31'.freeze
  FIELD_BID = '84'.freeze
  FIELD_ASK = '86'.freeze

  # Regional first-party OAuth self-service portals, keyed by EU brokerage entity. The connect
  # wizard sends the user to their entity's portal to register a consumer + upload the three
  # public artifacts. EU entities only — never the US ip2loc portal. Path/query/fragment verified
  # against the live IBIE portal; only the TLD changes per entity.
  OAUTH_PORTAL_PATH = '/oauth/?loginType=1&action=OAUTH&clt=0&RL=1#/configuration'.freeze
  OAUTH_PORTALS = {
    'ibie' => { name: 'IBKR Ireland (IBIE)', url: "https://www.interactivebrokers.ie#{OAUTH_PORTAL_PATH}" },
    'iblux' => { name: 'IBKR Luxembourg (IBLUX)', url: "https://www.interactivebrokers.lu#{OAUTH_PORTAL_PATH}" },
    'ibce' => { name: 'IBKR Central Europe (IBCE)', url: "https://www.interactivebrokers.com.hu#{OAUTH_PORTAL_PATH}" },
    'ibuk' => { name: 'IBKR U.K. (IBUK)', url: "https://www.interactivebrokers.co.uk#{OAUTH_PORTAL_PATH}" }
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
    @account_id = nil
    @client = Clients::Ibkr.new(api_key: api_key)
  end

  # IBKR paper vs live is determined by the (separate) credentials, not a host/mode switch.
  def requires_passphrase?
    false
  end

  def supports_withdrawal?
    false
  end

  def fetch_withdrawal_fees!
    Result::Success.new({})
  end

  # IBKR has no notional/cash-quantity order — every order is a whole-share quantity, so the
  # sizing logic must run in :base. minimum_base_size=1 / base_decimals=0 (set on the ticker)
  # makes the smart-intervals carry-forward accumulate until a whole share is affordable.
  def minimum_amount_logic(**)
    :base
  end

  # Fail-open for now (like Alpaca when its clock is unavailable). IBKR queues/rejects off-hours
  # orders itself; a real per-exchange equities calendar is a later refinement.
  def market_open?
    true
  end

  def next_market_open_at
    Time.current
  end

  # --- ordering (whole shares; the critical path) ---

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
    result = client.order_status(order_id: order_id)
    return result if result.failure?

    Result::Success.new(parse_order_data(result.data, order_id: order_id))
  end

  def get_orders(order_ids:)
    orders = {}
    missing = []
    Array(order_ids).each do |order_id|
      result = client.order_status(order_id: order_id)
      return result if result.failure?

      data = result.data
      data.present? ? orders[order_id] = parse_order_data(data, order_id: order_id) : missing << order_id
    end
    Result::Success.new(orders: orders, missing: missing)
  end

  def cancel_order(order_id:)
    acct = account_id
    return Result::Failure.new('No IBKR account available') if acct.blank?

    result = client.cancel_order(account_id: acct, order_id: order_id)
    return result if result.failure?

    Result::Success.new(order_id)
  end

  # --- balances ---

  def get_balances(asset_ids: nil)
    acct = account_id
    return Result::Failure.new('No IBKR account available') if acct.blank?

    ledger = client.ledger(account_id: acct)
    return ledger if ledger.failure?

    positions = client.positions(account_id: acct)
    return positions if positions.failure?

    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.index_with { { free: 0, locked: 0 } }
    apply_cash_balances(balances, ledger.data, asset_ids)
    apply_position_balances(balances, positions.data, asset_ids)
    Result::Success.new(balances)
  end

  # --- prices (order-critical via IBKR snapshot; cosmetic candles deferred to the data-api feed) ---

  def get_last_price(ticker:, force: false)
    snapshot_price(ticker: ticker, field: FIELD_LAST, label: 'last', force: force)
  end

  def get_bid_price(ticker:, force: false)
    snapshot_price(ticker: ticker, field: FIELD_BID, label: 'bid', force: force)
  end

  def get_ask_price(ticker:, force: false)
    snapshot_price(ticker: ticker, field: FIELD_ASK, label: 'ask', force: force)
  end

  def get_tickers_prices(force: false, symbols: nil)
    Result::Success.new({}) # tradeable prices come from the data-api feed (§7); refined from logs
  end

  def get_candles(ticker:, start_at:, timeframe:)
    Result::Success.new([]) # cosmetic history reuses the data-api EODHD path (§7)
  end

  def get_tickers_info(force: false)
    Result::Success.new([]) # the IBKR catalog is data-api driven (§6), not per-user fetched
  end

  # --- validity / ledger ---

  def get_api_key_validity(api_key:)
    result = Clients::Ibkr.new(api_key: api_key).accounts
    return Result::Success.new(:pending_activation) if result.failure? # registered but not yet usable

    Result::Success.new(extract_accounts(result.data).any? || :pending_activation)
  rescue Client::TransientNetworkError => e
    Result::Failure.new(e.message)
  end

  # Required by the nightly AccountTransaction::SyncAllJob. Returns [] for now (safe no-op so the
  # job never raises); real transaction normalization is refined once the live shapes are captured.
  def get_ledger(api_key:, start_time: nil)
    Rails.logger.info("[IBKR] get_ledger noop (api_key=#{api_key.id}) — transaction sync pending live shapes")
    Result::Success.new([])
  end

  private

  def account_id
    return @account_id if defined?(@account_id) && @account_id

    result = client.accounts
    if result.failure?
      Rails.logger.warn("[IBKR] account discovery failed: #{result.errors.to_sentence}")
      return nil
    end
    @account_id = extract_accounts(result.data).first
  end

  def extract_accounts(data)
    return [] unless data

    selected = data.is_a?(Hash) ? data['selectedAccount'] : nil
    accounts = data.is_a?(Hash) ? Array(data['accounts']) : Array(data)
    [selected, *accounts].compact.uniq
  end

  # set_market_order / set_limit_order are the names Exchange::Dryable decorates, so dry-run mode
  # is honoured automatically (it must never reach the live IBKR API).
  def set_market_order(ticker:, amount:, amount_type:, side:)
    submit_order(ticker: ticker, amount: amount, amount_type: amount_type, side: side, order_type: 'MKT')
  end

  def set_limit_order(ticker:, amount:, amount_type:, side:, price:)
    submit_order(ticker: ticker, amount: amount, amount_type: amount_type, side: side, order_type: 'LMT', price: price)
  end

  def submit_order(ticker:, amount:, amount_type:, side:, order_type:, price: nil)
    qty = whole_shares(ticker: ticker, amount: amount, amount_type: amount_type, price: price)
    return Result::Failure.new("Order below minimum (0 shares) for #{ticker.base}") if qty < 1

    conid = resolve_conid(ticker)
    return conid if conid.is_a?(Result) # search_contract failure

    acct = account_id
    return Result::Failure.new('No IBKR account available') if acct.blank?

    result = client.place_order(
      account_id: acct, conid: conid, side: side, quantity: qty,
      order_type: order_type, price: price ? formatted_price(ticker, price) : nil
    )
    return result if result.failure?

    order_id = extract_order_id(result.data)
    return Result::Failure.new("IBKR accepted the order but returned no order id for #{ticker.base}") if order_id.blank?

    Result::Success.new({ order_id: order_id })
  end

  def whole_shares(ticker:, amount:, amount_type:, price:)
    base_amount = if amount_type == :quote
                    price.to_d.positive? ? (amount.to_d / price.to_d) : 0
                  else
                    amount.to_d
                  end
    ticker.adjusted_amount(amount: base_amount, amount_type: :base).to_i
  end

  def resolve_conid(ticker)
    cached = ticker.try(:conid)
    return cached.to_i if cached.present?

    result = client.search_contract(symbol: ticker.base, currency: ticker.quote)
    result.success? ? result.data : result
  end

  def formatted_price(ticker, price)
    format("%.#{ticker.price_decimals}f", ticker.adjusted_price(price: price).to_d)
  end

  # Only a confirmed order ack carries order_id/orderId — never fall back to a prompt's `id`.
  def extract_order_id(data)
    ack = Array(data).find { |e| e.is_a?(Hash) && (e['order_id'] || e['orderId']) }
    ack && (ack['order_id'] || ack['orderId']).to_s
  end

  def snapshot_price(ticker:, field:, label:, force:)
    # Dry-run must NOT establish a brokerage session / hit IBKR; a placeholder keeps dry orders
    # flowing. Real cosmetic pricing moves to the (non-IBKR) data-api feed in §7.
    return Result::Success.new(BigDecimal('1')) if dry_run?

    conid = resolve_conid(ticker)
    return conid if conid.is_a?(Result)

    cache_key = "exchange_#{id}_#{label}_price_#{ticker.id}"
    cached = force ? nil : Rails.cache.read(cache_key)
    return Result::Success.new(cached) if cached

    result = client.snapshot(conids: [conid], fields: [field])
    return result if result.failure?

    value = snapshot_field(result.data, conid, field)
    return Result::Failure.new("Missing/zero #{label} price for #{ticker.base}") if value.zero?

    Rails.cache.write(cache_key, value, expires_in: 5.seconds)
    Result::Success.new(value)
  end

  def snapshot_field(data, conid, field)
    row = Array(data).find { |r| r.is_a?(Hash) && r['conid'].to_i == conid.to_i } || Array(data).first
    row.is_a?(Hash) ? row[field].to_d : 0.to_d
  end

  def parse_order_data(data, order_id: nil)
    data = {} unless data.is_a?(Hash)
    filled = data['filledQuantity'].presence || data['cumFill'] || data['filled']
    filled = filled.to_d
    avg = (data['avgPrice'] || data['average_price']).to_d
    limit = (data['price'] || data['limit_price']).to_d

    {
      order_id: (data['order_id'] || data['orderId'] || order_id).to_s,
      ticker: ticker_for(data),
      price: avg.positive? ? avg : limit,
      amount: (data['totalSize'] || data['quantity']).to_d,
      quote_amount: nil,
      amount_exec: filled,
      quote_amount_exec: filled * avg,
      side: data['side']&.to_s&.downcase&.to_sym,
      order_type: (data['order_type'] || data['orderType']).to_s.casecmp?('LMT') ? :limit_order : :market_order,
      error_messages: [],
      status: parse_order_status(data['order_status'] || data['status']),
      exchange_response: data
    }
  end

  def parse_order_status(status)
    case status.to_s
    when 'Filled' then :closed
    when 'Cancelled', 'PendingCancel', 'Inactive' then :cancelled
    when 'Submitted', 'PreSubmitted', 'PendingSubmit' then :open
    when 'Rejected' then :failed
    else :unknown
    end
  end

  def ticker_for(data)
    if data['conid'].present? && Ticker.column_names.include?('conid')
      by_conid = tickers.find_by(conid: data['conid'])
      return by_conid if by_conid
    end
    tickers.find_by(base: data['ticker'] || data['symbol'])
  end

  def apply_cash_balances(balances, ledger_data, asset_ids)
    return unless ledger_data.is_a?(Hash)

    ledger_data.each do |currency, row|
      next unless row.is_a?(Hash)

      asset = asset_from_symbol(currency.to_s.upcase)
      next unless asset && asset_ids.include?(asset.id)

      balances[asset.id] = { free: row['cashbalance'].to_d, locked: 0 }
    end
  end

  def apply_position_balances(balances, positions_data, asset_ids)
    Array(positions_data).each do |position|
      next unless position.is_a?(Hash)

      asset = asset_from_conid(position['conid']) || asset_from_symbol(position['ticker'] || position['contractDesc'])
      next unless asset && asset_ids.include?(asset.id)

      balances[asset.id] = { free: position['position'].to_d, locked: 0 }
    end
  end

  def asset_from_conid(conid)
    return nil if conid.blank? || !Ticker.column_names.include?('conid')

    tickers.available.find_by(conid: conid)&.base_asset
  end
end
