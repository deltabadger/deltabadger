# The Rails half of the sync row-parity harness (rust/tests/sync_parity.rs is the Rust half).
#   bin/rails runner script/rust/sync.rb grid <root>    # one install per scenario, built with Rails' own models
#   bin/rails runner script/rust/sync.rb record <root>  # Rails' real jobs per <root>/<scenario>/ -> rails.json
# Always run with every *_DATABASE_URL pointing at scratch files. Alpaca and the hosted market-data API are scripted
# beneath Rails' own clients, at the Faraday adapter (the contract of script/rust/decisions.rb's ScriptedAlpaca, plus a
# request log and the market-data host): every line of Clients::Alpaca, Clients::MarketData, Exchanges::Alpaca,
# AccountTransactionSync and AccountBalance::Sync runs. A call a step did not script raises Harness::Unscripted, and so
# does any other real connection.
require 'json'
require 'fileutils'
require 'active_support/testing/time_helpers'
require_relative 'schema_template'

# A hosted instance: prices that are neither cash nor Alpaca's own come from the market-data API (MarketDataSettings).
ENV['MARKET_DATA_URL'] = 'http://data-api:3000'
ENV['MARKET_DATA_TOKEN'] = 'parity-token'

module Harness
  # An Exception, not a StandardError: with_rescue and the jobs' own rescues would turn a harness gap into a sync failure.
  class Unscripted < Exception; end # rubocop:disable Lint/InheritException
end

Net::HTTP.prepend(Module.new { def connect = raise(Harness::Unscripted, "real connection to #{address}:#{port}") })
# No CoinGecko key from a local .env: with one, Clients::Coingecko.detect_plan probes pro-api before any scripted call.
ENV['COINGECKO_API_KEY'] = ''

module ScriptedSync
  mattr_accessor :alpaca, :binance, :binance_state, :kucoin, :bitvavo, :gemini, :bitget, :market, :requests, :filters_after

  NETWORK = {
    'pre_send' => -> { Faraday::ConnectionFailed.new(Errno::ECONNREFUSED.new('connect(2) for "paper-api.alpaca.markets" port 443')) },
    'post_send' => -> { Faraday::TimeoutError.new(Net::ReadTimeout.new) },
    'permanent' => -> { Faraday::SSLError.new(OpenSSL::SSL::SSLError.new('certificate verify failed')) },
    # Honeymaker reports e.message alone (Honeymaker::Client#with_rescue), and returns it: nothing raises.
    'binance_pre_send' => -> { Faraday::ConnectionFailed.new(Errno::ECONNREFUSED.new('connect(2) for "api.binance.com" port 443')) },
    'kucoin_pre_send' => -> { Faraday::ConnectionFailed.new(Errno::ECONNREFUSED.new('connect(2) for "api.kucoin.com" port 443')) },
    'bitvavo_pre_send' => -> { Faraday::ConnectionFailed.new(Errno::ECONNREFUSED.new('connect(2) for "api.bitvavo.com" port 443')) },
    'gemini_pre_send' => -> { Faraday::ConnectionFailed.new(Errno::ECONNREFUSED.new('connect(2) for "api.gemini.com" port 443')) },
    'bitget_pre_send' => -> { Faraday::ConnectionFailed.new(Errno::ECONNREFUSED.new('connect(2) for "api.bitget.com" port 443')) }
  }.freeze

  def self.reply(env)
    host = env.url.host.to_s
    if binance_state && (%w[binance.com binance.us].any? { |d| host == d || host.end_with?(".#{d}") })
      requests << ["#{env.method.to_s.upcase} #{env.url.path}", URI.decode_www_form(env.url.query.to_s).sort]
      return FakeBinance.reply(binance_state, env.url.path, URI.decode_www_form(env.url.query.to_s).to_h)
    end
    script = if host.end_with?('alpaca.markets') then alpaca
             elsif host.start_with?('api.binance.') then binance
             elsif host == 'api.kucoin.com' then kucoin
             elsif host == 'api.bitvavo.com' then bitvavo
             elsif host == 'api.gemini.com' then gemini
             elsif host == 'api.bitget.com' then bitget
             elsif host == 'data-api' then market
             end
    key = "#{env.method.to_s.upcase} #{env.url.path}"
    raise Harness::Unscripted, "unscripted HTTP call #{key} to #{host}" if script.nil?

    requests << [key, host == 'api.gemini.com' ? gemini_params(env) : URI.decode_www_form(env.url.query.to_s).sort]
    queue = script[key] or raise Harness::Unscripted, "unscripted call #{key}"
    queue.size > 1 ? queue.shift : queue.first
  end

  # Gemini's params travel in its signed payload (X-GEMINI-PAYLOAD; a public read has none): logged as a query's pairs,
  # without `request` (the path) and `nonce` (the clock).
  def self.gemini_params(env)
    payload = env.request_headers['X-GEMINI-PAYLOAD']
    payload ? JSON.parse(Base64.strict_decode64(payload)).except('request', 'nonce').map { |k, v| [k, v.to_s] }.sort : []
  end

  # Where a step says so (`server_filters_after`), an activities page is served as Alpaca serves it: only what is later
  # than the request's `after`. The Rust half filters the same way (rust/src/sync/parity.rs), so a scenario can show
  # what each side's `after` would and would not bring back.
  def self.served(env, body)
    after = URI.decode_www_form(env.url.query.to_s).to_h['after']
    return body unless filters_after && after && body.is_a?(Array) && env.url.path == '/v2/account/activities'

    body.select do |activity|
      time = activity['transaction_time'] || activity['date']
      time.nil? || Time.zone.parse(time) > Time.iso8601(after)
    end
  end

  module Adapter
    def call(env)
      reply = ScriptedSync.reply(env)
      if (kind = reply['network'])
        error = ScriptedSync::NETWORK.fetch(kind).call
        actual = kind.start_with?('binance_', 'kucoin_', 'bitvavo_', 'gemini_', 'bitget_') ? error.message : "#{error.class}: #{error.message}" # what the venue's client reports
        raise Harness::Unscripted, "#{kind}: the script says #{reply['message'].inspect}, Ruby says #{actual.inspect}" unless actual == reply['message']

        raise error
      end
      body = reply['body'].is_a?(String) ? reply['body'] : JSON.generate(ScriptedSync.served(env, reply['body']))
      env.response = Faraday::Response.new
      save_response(env, reply.fetch('status', 200), body, { 'Content-Type' => 'application/json' })
      @app.call(env)
    end
  end
end

# A Binance account, served from a scenario's `binance_state` as Binance serves it, whatever each side asks: the
# histories by their documented windows and pages, the trade list by `fromId` (trades >= it, by id), by `startTime`
# (from it on, by id; ponytail: the docs do not say what an open-ended startTime returns, so nothing is cut off at
# 24 hours here) or else the most recent `limit`. Rust's half (rust/src/sync/parity.rs FakeBinance) serves the same.
# Converts, fiat payments, dust, dividends and Simple Earn are served from the state by each row's own time, and paged
# where Binance pages; margin and futures (not read by Rust, ruling Q11) answer empty.
module FakeBinance
  module_function

  EMPTY = {
    '/sapi/v1/margin/interestHistory' => { 'rows' => [], 'total' => 0 }, '/sapi/v1/margin/forceLiquidationRec' => { 'rows' => [], 'total' => 0 },
    '/fapi/v1/income' => [], '/dapi/v1/income' => []
  }.freeze

  def applied(text) = (Time.utc(*text.scan(/\d+/).map(&:to_i)).to_r * 1000).to_i

  # What Binance refuses before reading anything (rust/src/sync/parity.rs binance_refusal is the same table): a missing
  # mandatory parameter (its docs, and `type` on flexible rewards, which Binance requires though its docs say
  # otherwise), a signed path without its timestamp, and a window longer than the endpoint accepts.
  MANDATORY = {
    '/api/v3/exchangeInfo' => [], '/api/v3/myTrades' => %w[symbol timestamp], '/sapi/v1/convert/tradeFlow' => %w[startTime endTime timestamp],
    '/sapi/v1/fiat/payments' => %w[transactionType timestamp], '/sapi/v1/simple-earn/flexible/history/rewardsRecord' => %w[type timestamp]
  }.freeze

  def max_days(path)
    case path
    when '/sapi/v1/capital/deposit/hisrec', '/sapi/v1/capital/withdraw/history' then 90
    when '/sapi/v1/convert/tradeFlow', %r{\A/sapi/v1/simple-earn/} then 30
    when '/sapi/v1/asset/assetDividend' then 180
    when '/api/v3/myTrades' then 1
    end
  end

  def refusal(path, query)
    missing = MANDATORY.fetch(path, %w[timestamp]).find { |k| query[k].to_s.empty? }
    return { 'status' => 400, 'body' => { 'code' => -1102, 'msg' => "Mandatory parameter '#{missing}' was not sent, was empty/null, or malformed." } } if missing

    days = max_days(path)
    return unless days && query['startTime'] && query['endTime'] && query['endTime'].to_i - query['startTime'].to_i > days * 86_400_000

    { 'status' => 400, 'body' => { 'code' => -1127, 'msg' => "More than #{days * 24} hours between startTime and endTime." } }
  end

  def reply(state, path, query)
    refused = refusal(path, query)
    return refused if refused

    failures = state.fetch('fail', {})
    failure = failures["#{path}?symbol=#{query['symbol']}"] || failures[path]
    return failure if failure

    body = case path
           when '/api/v3/account'
             { 'accountType' => 'SPOT', 'balances' => state.fetch('balances', []).map { |a, f, l| { 'asset' => a, 'free' => f, 'locked' => l || '0.00000000' } } }
           when '/api/v3/exchangeInfo'
             { 'timezone' => 'UTC', 'symbols' => state.fetch('symbols', []).map { |s, b, q, st| { 'symbol' => s, 'status' => st || 'TRADING', 'baseAsset' => b, 'quoteAsset' => q } } }
           when '/sapi/v1/capital/deposit/hisrec' then window(state.fetch('deposits', []), query) { |d| d['insertTime'] }
           when '/sapi/v1/capital/withdraw/history' then window(state.fetch('withdrawals', []), query) { |w| applied(w['applyTime']) }
           when '/api/v3/myTrades' then trades(state.fetch('trades', {}).fetch(query['symbol'], []), query)
           when '/sapi/v1/convert/tradeFlow' then { 'list' => window(state.fetch('converts', []), query) { |c| c['createTime'] }, 'moreData' => false }
           when '/sapi/v1/fiat/payments'
             rows = between(state.fetch(query['transactionType'] == '1' ? 'fiat_sells' : 'fiat_buys', []), query['beginTime'], query['endTime']) { |p| p['createTime'] }
             size = query.fetch('rows', '100').to_i
             { 'code' => '000000', 'message' => 'success', 'total' => rows.size, 'success' => true,
               'data' => rows.drop((query.fetch('page', '1').to_i - 1) * size).first(size) }
           when '/sapi/v1/asset/dribblet'
             rows = between(state.fetch('dust', []), query['startTime'], query['endTime']) { |d| d['operateTime'] }
             { 'total' => rows.size, 'userAssetDribblets' => rows }
           when '/sapi/v1/asset/assetDividend'
             rows = window(state.fetch('dividends', []), query) { |d| d['divTime'] }
             { 'rows' => rows, 'total' => rows.size }
           when %r{\A/sapi/v1/simple-earn/}
             key = path.delete_prefix('/sapi/v1/simple-earn/').sub('/history/', '_')
             rows = between(state.fetch('earn', {}).fetch(key, []), query['startTime'], query['endTime']) { |r| r['time'] || r['deliverDate'] || r['createTime'] }
             rows = rows.select { |r| r['type'] == query['type'] } if query['type'] && query['type'] != 'ALL'
             size = query.fetch('size', '10').to_i
             { 'rows' => rows.drop((query.fetch('current', '1').to_i - 1) * size).first(size), 'total' => rows.size }
           else EMPTY.fetch(path) { raise Harness::Unscripted, "fake Binance has no #{path}" }
           end
    { 'status' => 200, 'body' => body }
  end

  def between(rows, from, to)
    rows.select { |r| (t = yield(r)) && (from.nil? || t >= from.to_i) && (to.nil? || t <= to.to_i) }
  end

  def window(rows, query)
    from = query['startTime']&.to_i
    to = query['endTime']&.to_i
    rows.select { |r| (t = yield(r)) && (from.nil? || t >= from) && (to.nil? || t <= to) }
        .drop(query.fetch('offset', '0').to_i).first(query.fetch('limit', '1000').to_i)
  end

  def trades(rows, query)
    limit = query.fetch('limit', '500').to_i
    sorted = rows.sort_by { |t| t['id'] }
    if query['fromId'] then sorted.select { |t| t['id'] >= query['fromId'].to_i }.first(limit)
    elsif query['startTime'] then sorted.select { |t| t['time'] >= query['startTime'].to_i }.first(limit)
    else sorted.last(limit)
    end
  end
end

module SyncParity
  extend ActiveSupport::Testing::TimeHelpers
  module_function

  TABLES = %w[account_transactions account_balances api_keys bots bot_activity_logs].freeze
  JSON_COLUMNS = %w[raw_data manual_values settings transient_data details].freeze
  # Ciphertext (a fresh IV per write) and nothing a sync touches.
  SECRET_COLUMNS = %w[key secret passphrase access_token rsa_signature_key rsa_encryption_key dh_param].freeze
  KEY = { 'key' => 'PKPARITY7KEY4TEST2ID', 'secret' => 'parity-secret-Zq8fW3nL5xT1vB7mK2dR9hY4', 'passphrase' => 'paper' }.freeze

  PRE_SEND = { 'network' => 'pre_send', 'message' => 'Faraday::ConnectionFailed: Connection refused - connect(2) for "paper-api.alpaca.markets" port 443' }.freeze
  POST_SEND = { 'network' => 'post_send', 'message' => 'Faraday::TimeoutError: Net::ReadTimeout' }.freeze
  CERTIFICATE = { 'network' => 'permanent', 'message' => 'Faraday::SSLError: certificate verify failed' }.freeze
  UNAUTHORIZED = { 'status' => 401, 'body' => { 'code' => 40_110_000, 'message' => 'unauthorized.' } }.freeze
  SERVER_ERROR = { 'status' => 500, 'body' => { 'code' => 50_010_000, 'message' => 'internal server error' } }.freeze
  ACTIVITIES = 'GET /v2/account/activities'.freeze
  ACCOUNT = 'GET /v2/account'.freeze
  POSITIONS = 'GET /v2/positions'.freeze
  SNAPSHOTS = 'GET /v2/stocks/snapshots'.freeze
  PRICES = 'GET /api/v1/prices'.freeze

  def raw(value) = value.is_a?(Float) ? { 'f' => [value].pack('G').unpack1('H*') } : value

  def rows(table)
    ActiveRecord::Base.connection.select_all("SELECT * FROM #{table} ORDER BY id").to_a.to_h do |r|
      r = r.except(*SECRET_COLUMNS).transform_values { |v| raw(v) }
      # raw_data is compared twice: as the text in the column, byte for byte, and parsed (for what reads it).
      r['raw_data_text'] = r['raw_data'] if r.key?('raw_data')
      JSON_COLUMNS.each { |c| r[c] = JSON.parse(r[c]) if r[c].is_a?(String) }
      [r['id'], r]
    end
  end

  def snapshot = TABLES.to_h { |t| [t, rows(t)] }

  # Changed and new rows, then removed ones (after: nil), each list in id order.
  def diff(before, after)
    TABLES.to_h do |t|
      changed = after[t].filter_map { |id, row| row == before[t][id] ? nil : { 'id' => id, 'before' => before[t][id], 'after' => row } }
      removed = before[t].filter_map { |id, row| after[t].key?(id) ? nil : { 'id' => id, 'before' => row, 'after' => nil } }
      [t, changed + removed]
    end
  end

  def ok(body) = { 'status' => 200, 'body' => body }

  # ---- recorded shapes (docs.alpaca.markets/reference/getaccountactivities; the account and position bodies follow the
  # paper recordings of 2026-10-02: every number is a string) ----

  def fill(id, symbol, side, qty, price, time, order_id: "order-#{id}", **extra)
    { 'id' => id, 'activity_type' => 'FILL', 'transaction_time' => time, 'type' => 'fill', 'price' => price, 'qty' => qty,
      'side' => side, 'symbol' => symbol, 'leaves_qty' => '0', 'order_id' => order_id, 'cum_qty' => qty,
      'order_status' => 'filled' }.merge(extra.transform_keys(&:to_s))
  end

  # A non-trade activity. Keys given as nil are kept as JSON null; keys not given are absent.
  def nta(id, type, **fields)
    activity = { 'id' => id, 'activity_type' => type }.merge(fields.transform_keys(&:to_s))
    activity.key?('status') ? activity : activity.merge('status' => 'executed')
  end

  def account(cash, **extra)
    ok({ 'id' => 'paper-account', 'account_number' => 'PA0000000000', 'status' => 'ACTIVE', 'currency' => 'USD', 'cash' => cash,
         'buying_power' => '200000', 'non_marginable_buying_power' => '100000', 'portfolio_value' => '100000' }.merge(extra.transform_keys(&:to_s)))
  end

  # `available`: Alpaca's qty_available, the shares no open order holds.
  def position(symbol, qty, asset_class: 'us_equity', available: qty)
    { 'asset_id' => "asset-#{symbol}", 'symbol' => symbol, 'exchange' => asset_class == 'crypto' ? 'CRYPTO' : 'NASDAQ', 'asset_class' => asset_class,
      'asset_marginable' => asset_class != 'crypto', 'qty' => qty, 'qty_available' => available, 'avg_entry_price' => '100', 'side' => 'long',
      'market_value' => '0', 'cost_basis' => '0', 'current_price' => '100' }
  end

  def snapshot_body(prices)
    ok(prices.transform_values { |p| p.nil? ? {} : { 'latestTrade' => { 'p' => p, 's' => 100, 't' => '2026-09-18T19:59:59.5Z', 'x' => 'V' }, 'dailyBar' => { 'c' => p } } })
  end

  def prices_body(prices) = ok('data' => prices.transform_values { |p| { 'usd' => p } })

  # ---- the install ----

  def build(dir, scenario)
    SchemaTemplate.install(dir)
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
    travel_to(Time.utc(2026, 9, 1, 9, 0, 0)) do
      user = User.new(name: 'Owner', email: 'owner@example.com', encrypted_password: SchemaTemplate.owner_password('correct horse battery staple'), admin: true,
                      confirmed_at: Time.current, setup_completed: true)
      user.save!(validate: false)
      alpaca = Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25')
      # As the hosted reference-data jobs leave them: USD is the local `usd` row (category Fiat), stocks and ETFs are
      # category Stock, coins carry their CoinGecko id.
      assets = {
        'USD' => Asset.create!(external_id: 'usd', symbol: 'USD', name: 'US Dollar', category: 'Fiat'),
        'AAPL' => Asset.create!(external_id: 'AAPL.US', symbol: 'AAPL', name: 'Apple', category: 'Stock', instrument_type: 'stock'),
        'KLAC' => Asset.create!(external_id: 'KLAC.US', symbol: 'KLAC', name: 'KLA', category: 'Stock', instrument_type: 'stock'),
        'QQQM' => Asset.create!(external_id: 'QQQM.US', symbol: 'QQQM', name: 'Invesco NASDAQ 100', category: 'Stock', instrument_type: 'etf'),
        'BTC' => Asset.create!(external_id: 'bitcoin', symbol: 'BTC', name: 'Bitcoin', category: 'Cryptocurrency'),
        'ETH' => Asset.create!(external_id: 'ethereum', symbol: 'ETH', name: 'Ethereum', category: 'Cryptocurrency')
      }
      assets.each_value { |a| ExchangeAsset.create!(exchange: alpaca, asset: a, available: true) }
      stock = { base_decimals: 9, quote_decimals: 2, price_decimals: 2, minimum_base_size: '0.000000001', minimum_quote_size: '1' }
      %w[AAPL KLAC QQQM].each do |s|
        Ticker.create!(exchange: alpaca, ticker: s, base: s, quote: 'USD', base_asset: assets[s], quote_asset: assets['USD'], **stock)
      end
      %w[BTC ETH].each do |s|
        Ticker.create!(exchange: alpaca, ticker: "#{s}/USD", base: s, quote: 'USD', base_asset: assets[s], quote_asset: assets['USD'],
                       base_decimals: 9, quote_decimals: 2, price_decimals: 2, minimum_base_size: '0.000027', minimum_quote_size: '1')
      end
      key = ApiKey.new(user:, exchange: alpaca, **KEY.symbolize_keys, status: :correct, key_type: :trading)
      key.save!(validate: false)
      # The existing tracker fixture also uses Rails' supported plaintext placeholders,
      # letting both halves read identical credentials without the Rails test secret.
      ActiveRecord::Base.connection.exec_update('UPDATE api_keys SET key = ?, secret = ?, passphrase = ? WHERE id = ?', 'plain credentials',
                                                [KEY['key'], KEY['secret'], KEY['passphrase'], key.id])
      ctx = { user:, alpaca:, assets:, key: }
      scenario[:setup]&.call(ctx)
      ctx
    end
  end

  # A bot with one closed buy of `symbol`, placed at `at` (insert!: no callbacks, no jobs). The order is on `venue`.
  def holder(ctx, symbol, at:, amount: '10', external_id: nil, side: 0, venue: ctx[:alpaca])
    bot = ctx[:user].bots.new(type: 'Bots::DcaMultiAsset', exchange: ctx[:alpaca], settings: {
      'quote_asset_id' => ctx[:assets]['USD'].id, 'quote_amount' => 60.0, 'interval' => 'week', 'weighting' => 'manual',
      'allocations' => { ctx[:assets].fetch(symbol).id.to_s => 1.0 }
    })
    bot.set_missed_quote_amount
    bot.save!
    Transaction.insert!({ 'bot_id' => bot.id, 'exchange_id' => venue.id, 'base_asset_id' => ctx[:assets].fetch(symbol).id,
                          'quote_asset_id' => ctx[:assets]['USD'].id, 'base' => symbol, 'quote' => 'USD', 'side' => side, 'status' => 0,
                          'external_status' => 2, 'external_id' => external_id || "order-#{bot.id}", 'order_type' => 0, 'price' => '100',
                          'amount' => amount, 'amount_exec' => amount, 'quote_amount_exec' => (amount.to_d * 100).to_s('F'),
                          'transaction_type' => 'REGULAR', 'bot_interval' => 'week', 'bot_quote_amount' => 60.0, 'error_messages' => [],
                          'created_at' => at, 'updated_at' => at })
    bot
  end

  def stored(ctx, exchange: ctx[:alpaca], key: ctx[:key], **attrs)
    AccountTransaction.insert!({ user_id: ctx[:user].id, exchange_id: exchange.id, api_key_id: key&.id, raw_data: {}, manual_values: {},
                                 created_at: Time.current, updated_at: Time.current }.merge(attrs))
  end

  def balance(ctx, symbol, free:, usd_price: nil, usd_value: nil, priced_at: nil, exchange: ctx[:binance] || ctx[:alpaca], user: ctx[:user])
    at = Time.utc(2026, 9, 10, 2, 30, 0)
    AccountBalance.insert!({ user_id: user.id, exchange_id: exchange.id, asset_id: ctx[:assets].fetch(symbol).id, free:, locked: 0,
                             usd_price:, usd_value:, priced_at: priced_at || (usd_price && at), synced_at: at, created_at: at, updated_at: at })
  end

  def ledger(at, *pages) = { 'kind' => 'ledger', 'at' => at, 'alpaca' => { ACTIVITIES => pages.map { |p| p.is_a?(Array) ? ok(p) : p } } }

  # The assets a venue install may add beside the Alpaca one's: [external_id, name, category].
  VENUE_ASSETS = { 'USDT' => %w[tether Tether Cryptocurrency], 'EUR' => %w[eur Euro Fiat] }.freeze

  # A venue's install beside the Alpaca one (plan VB §1.8, one builder for every venue's sync scenarios): the exchange row,
  # the `assets` it lists (any missing from VENUE_ASSETS created first, in that order), its `pairs` as `pair` spells them
  # with the `coin` precisions, and a reading key that becomes the scenario's key, saved under `ctx[as]`.
  def venue(ctx, type, name, as:, assets:, pairs:, coin:, pair:)
    venue = type.create!(name:, maker_fee: '0.1', taker_fee: '0.1')
    all = ctx[:assets]
    assets.each do |s|
      spec = VENUE_ASSETS[s] or next
      all[s] ||= Asset.create!(external_id: spec[0], symbol: s, name: spec[1], category: spec[2])
    end
    assets.each { |s| ExchangeAsset.create!(exchange: venue, asset: all[s], available: true) }
    pairs.each do |base, quote|
      Ticker.create!(exchange: venue, ticker: pair.(base, quote), base:, quote:, base_asset: all[base], quote_asset: all[quote], **coin)
    end
    key = ApiKey.new(user: ctx[:user], exchange: venue, **KEY.symbolize_keys, status: :correct, key_type: :read_only)
    key.save!(validate: false)
    ActiveRecord::Base.connection.exec_update('UPDATE api_keys SET key = ?, secret = ?, passphrase = ? WHERE id = ?', 'plain credentials',
                                              [KEY['key'], KEY['secret'], KEY['passphrase'], key.id])
    ctx[:key] = key
    ctx[as] = venue
  end

  # A Binance or Binance.US install. EUR is cash (category Fiat) priced through the market API's exchange rates.
  def binance(ctx, us: false)
    venue(ctx, us ? Exchanges::BinanceUs : Exchanges::Binance, us ? 'Binance.US' : 'Binance', as: :binance, assets: %w[BTC ETH USDT EUR],
          pairs: [%w[BTC USDT], %w[ETH USDT], %w[BTC EUR], %w[ETH BTC]], pair: ->(b, q) { "#{b}#{q}" },
          coin: { base_decimals: 8, quote_decimals: 8, price_decimals: 2, minimum_base_size: '0.00001', minimum_quote_size: '5' })
  end

  # A KuCoin key on the classic (trade) account, with the pairs its fills name.
  def kucoin(ctx)
    venue(ctx, Exchanges::Kucoin, 'KuCoin', as: :kucoin, assets: %w[BTC ETH USDT], pairs: [%w[BTC USDT], %w[ETH USDT], %w[ETH BTC]],
          pair: ->(b, q) { "#{b}-#{q}" }, coin: { base_decimals: 8, quote_decimals: 6, price_decimals: 1, minimum_base_size: '0.00001', minimum_quote_size: '0.1' })
  end

  # A Gemini key, with the pairs its trades name (lowercase, as Exchanges::Gemini#get_tickers_info stores them).
  def gemini(ctx)
    venue(ctx, Exchanges::Gemini, 'Gemini', as: :gemini, assets: %w[BTC ETH USD], pairs: [%w[BTC USD], %w[ETH USD], %w[ETH BTC]],
          pair: ->(b, q) { "#{b}#{q}".downcase }, coin: { base_decimals: 8, quote_decimals: 2, price_decimals: 2, minimum_base_size: '0.00001', minimum_quote_size: '0' })
  end
  def gm_trade(tid, symbol, type, amount, price, at, fee: '0.06', fee_currency: 'USD')
    { 'price' => price, 'amount' => amount, 'timestamp' => (at.is_a?(Integer) ? at : ms(at)) / 1000, 'timestampms' => at.is_a?(Integer) ? at : ms(at),
      'type' => type, 'aggressor' => true, 'fee_currency' => fee_currency, 'fee_amount' => fee, 'tid' => tid, 'order_id' => "o#{tid}",
      'exchange' => 'gemini', 'is_auction_fill' => false, 'is_clearing_fill' => false, 'symbol' => symbol }.compact
  end
  def gm_transfer(eid, type, currency, amount, at, fee: nil, fee_currency: nil)
    { 'type' => type, 'status' => 'Complete', 'timestampms' => at.is_a?(Integer) ? at : ms(at), 'eid' => eid, 'currency' => currency, 'amount' => amount,
      'method' => 'CryptoTransfer', 'txHash' => "0xg#{eid}", 'feeAmount' => fee, 'feeCurrency' => fee_currency }.compact
  end
  GM_TRADES = 'POST /v1/mytrades'.freeze
  GM_TRANSFERS = 'POST /v2/transfers'.freeze
  GM_BALANCES = 'POST /v1/balances'.freeze
  # Gemini's balance rows: [currency, available, amount] (amount = available when left out).
  def gemini_rows(*rows) = ok(rows.map { |c, available, amount| { 'type' => 'exchange', 'currency' => c, 'amount' => amount || available, 'available' => available, 'availableForWithdrawal' => available } })
  def gemini_balances(at, balances:, prices: nil, rates: nil)
    { 'kind' => 'balances', 'at' => at, 'gemini' => { GM_BALANCES => [balances] },
      'market' => { EXCHANGE_RATES => rates && [rates], PRICES => prices && [prices] }.compact }
  end
  # A ledger run over Gemini's scripted answers: each path's pages in order, the last one repeated.
  def gemini_ledger(at, trades: [ok([])], transfers: [ok([])])
    { 'kind' => 'ledger', 'at' => at, 'gemini' => { GM_TRADES => trades, GM_TRANSFERS => transfers } }
  end

  # A Bitget key (plan VB slice A4), with the pairs its fills name (BTCUSDT, as #get_tickers_info stores them).
  def bitget(ctx)
    venue(ctx, Exchanges::Bitget, 'Bitget', as: :bitget, assets: %w[BTC ETH USDT], pairs: [%w[BTC USDT], %w[ETH USDT], %w[ETH BTC]],
          pair: ->(b, q) { "#{b}#{q}" }, coin: { base_decimals: 6, quote_decimals: 8, price_decimals: 2, minimum_base_size: '0', minimum_quote_size: '1' })
  end
  BG_FILLS = 'GET /api/v2/spot/trade/fills'.freeze
  BG_DEPOSITS = 'GET /api/v2/spot/wallet/deposit-records'.freeze
  BG_WITHDRAWALS = 'GET /api/v2/spot/wallet/withdrawal-records'.freeze
  BG_ASSETS = 'GET /api/v2/spot/account/assets'.freeze
  def bg_page(rows) = ok('code' => '00000', 'msg' => 'success', 'requestTime' => 1_789_871_400_750, 'data' => rows)
  def bg_err(status, code, msg) = { 'status' => status, 'body' => { 'code' => code, 'msg' => msg, 'requestTime' => 1_789_871_400_750, 'data' => nil } }
  def bg_fill(trade, symbol, side, size, price, at, fee: '-0.06', fee_coin: 'USDT', order: nil, detail: true)
    at = at.is_a?(Integer) ? at : ms(at)
    { 'userId' => '1001', 'symbol' => symbol, 'orderId' => order || "o#{trade}", 'tradeId' => trade, 'orderType' => 'market', 'side' => side, 'priceAvg' => price,
      'size' => size, 'amount' => (BigDecimal(size) * BigDecimal(price)).to_s('F'), 'tradeScope' => 'taker',
      'feeDetail' => detail ? { 'deduction' => 'no', 'feeCoin' => fee_coin, 'totalDeductionFee' => '0', 'totalFee' => fee } : nil,
      'cTime' => at.to_s, 'uTime' => at.to_s }.compact
  end
  def bg_transfer(order, coin, size, at, status: 'success', fee: nil, trade: nil, type: 'deposit')
    at = at.is_a?(Integer) ? at : ms(at)
    { 'orderId' => order, 'tradeId' => trade || "0x#{order}", 'coin' => coin, 'type' => type, 'size' => size, 'status' => status, 'toAddress' => 'TXabc',
      'dest' => 'on_chain', 'chain' => 'TRC20', 'fee' => fee, 'cTime' => at.to_s, 'uTime' => at.to_s }.compact
  end
  # Bitget's asset rows: [coin, available, frozen].
  def bg_assets(*rows) = bg_page(rows.map { |coin, available, frozen| { 'coin' => coin, 'available' => available, 'frozen' => frozen || '0', 'locked' => '0', 'uTime' => '1789871400750' } })
  # A ledger run over Bitget's scripted answers: each path's pages in order (every window's read takes the next), the last one
  # repeated.
  def bitget_ledger(at, fills: [bg_page([])], deposits: [bg_page([])], withdrawals: [bg_page([])])
    { 'kind' => 'ledger', 'at' => at, 'bitget' => { BG_FILLS => fills, BG_DEPOSITS => deposits, BG_WITHDRAWALS => withdrawals } }
  end
  def bitget_balances(at, assets:, prices: nil, rates: nil)
    { 'kind' => 'balances', 'at' => at, 'bitget' => { BG_ASSETS => [assets] },
      'market' => { EXCHANGE_RATES => rates && [rates], PRICES => prices && [prices] }.compact }
  end

  def kc_page(items, page: 1, pages: nil, total: nil)
    ok('code' => '200000', 'data' => { 'currentPage' => page, 'pageSize' => 500, 'totalNum' => total || items.size,
                                       'totalPage' => pages || (items.empty? ? 0 : 1), 'items' => items })
  end
  def kc_fill(trade, symbol, side, size, funds, at, fee: '0.06', fee_currency: 'USDT', order: "o#{trade}")
    { 'symbol' => symbol, 'tradeId' => trade, 'orderId' => order, 'side' => side, 'type' => 'market', 'price' => (funds.to_d / size.to_d).round(8).to_s('F'),
      'size' => size, 'funds' => funds, 'fee' => fee, 'feeCurrency' => fee_currency, 'createdAt' => at.is_a?(Integer) ? at : ms(at) }
  end
  def kc_transfer(id, currency, amount, at, fee: '0', wallet: "0xk#{id}")
    { 'id' => id, 'currency' => currency, 'amount' => amount, 'fee' => fee, 'walletTxId' => wallet, 'status' => 'SUCCESS',
      'createdAt' => at.is_a?(Integer) ? at : ms(at) }.compact
  end
  # A Bitvavo key (plan VB slice A3), with the markets its scenarios trade; EUR is cash, priced through the exchange rates.
  def bitvavo(ctx)
    venue(ctx, Exchanges::Bitvavo, 'Bitvavo', as: :bitvavo, assets: %w[BTC ETH EUR], pairs: [%w[BTC EUR], %w[ETH EUR], %w[ETH BTC]],
          pair: ->(b, q) { "#{b}-#{q}" }, coin: { base_decimals: 8, quote_decimals: 2, price_decimals: 0, minimum_base_size: '0.0001', minimum_quote_size: '5' })
  end
  BV_DEPOSITS = 'GET /v2/depositHistory'.freeze
  BV_WITHDRAWALS = 'GET /v2/withdrawalHistory'.freeze
  BV_BALANCE = 'GET /v2/balance'.freeze
  BV_MARKETS = 'GET /v2/markets'.freeze
  BV_TRADES = 'GET /v2/trades'.freeze
  def bv_transfer(symbol, amount, at, fee: '0', status: 'completed', tx: nil)
    { 'timestamp' => at.is_a?(Integer) ? at : ms(at), 'symbol' => symbol, 'amount' => amount, 'fee' => fee, 'status' => status, 'txId' => tx }.compact
  end
  def bv_trade(id, market, side, amount, price, at, fee: '0.15', fee_currency: 'EUR')
    { 'id' => id, 'orderId' => "o-#{id}", 'timestamp' => at.is_a?(Integer) ? at : ms(at), 'market' => market, 'side' => side, 'amount' => amount,
      'price' => price, 'taker' => true, 'fee' => fee, 'feeCurrency' => fee_currency, 'settled' => true }
  end
  def bv_markets(*markets) = ok(markets.map { |m| { 'market' => m, 'status' => 'trading', 'base' => m.split('-')[0], 'quote' => m.split('-')[1] } })
  def bv_balance(*rows) = ok(rows.map { |symbol, available, in_order| { 'symbol' => symbol, 'available' => available, 'inOrder' => in_order || '0' } })
  # A ledger run over Bitvavo's scripted answers: each path's replies in order, the last one repeated.
  def bitvavo_ledger(at, deposits: [ok([])], withdrawals: [ok([])], balance: [bv_balance], markets: [bv_markets('BTC-EUR')], trades: [ok([])])
    { 'kind' => 'ledger', 'at' => at, 'bitvavo' => { BV_DEPOSITS => deposits, BV_WITHDRAWALS => withdrawals, BV_BALANCE => balance, BV_MARKETS => markets,
                                                     BV_TRADES => trades } }
  end
  def bitvavo_balances(at, balance:, prices: nil, rates: nil)
    { 'kind' => 'balances', 'at' => at, 'bitvavo' => { BV_BALANCE => [balance] },
      'market' => { EXCHANGE_RATES => rates && [rates], PRICES => prices && [prices] }.compact }
  end
  KC_FILLS = 'GET /api/v1/fills'.freeze
  KC_DEPOSITS = 'GET /api/v1/deposits'.freeze
  KC_WITHDRAWALS = 'GET /api/v1/withdrawals'.freeze
  KC_ACCOUNTS = 'GET /api/v1/accounts'.freeze
  # KuCoin's trade-account rows: [currency, available, holds] (holds 0 when left out); `locked:` names the column `locked`.
  def kucoin_accounts(*rows, locked: false)
    ok('code' => '200000', 'data' => rows.map do |currency, available, held|
      { 'id' => "a-#{currency}", 'currency' => currency, 'type' => 'trade', 'balance' => available, 'available' => available,
        (locked ? 'locked' : 'holds') => held || '0' }
    end)
  end
  # A balances run over KuCoin's scripted accounts answer, with the market API's prices and rates.
  def kucoin_balances(at, accounts:, prices: nil, rates: nil)
    { 'kind' => 'balances', 'at' => at, 'kucoin' => { KC_ACCOUNTS => [accounts] },
      'market' => { EXCHANGE_RATES => rates && [rates], PRICES => prices && [prices] }.compact }
  end
  # A ledger run over KuCoin's scripted answers: each path's replies in order, the last one repeated.
  def kucoin_ledger(at, fills: [kc_page([])], deposits: [kc_page([])], withdrawals: [kc_page([])])
    { 'kind' => 'ledger', 'at' => at, 'kucoin' => { KC_FILLS => fills, KC_DEPOSITS => deposits, KC_WITHDRAWALS => withdrawals } }
  end

  def ms(time) = (time.to_r * 1000).to_i

  def bn_trade(id, order, price, qty, at, buyer:, commission: '0.00000000', asset: 'BNB')
    { 'id' => id, 'orderId' => order, 'orderListId' => -1, 'price' => price, 'qty' => qty,
      'quoteQty' => (price.to_d * qty.to_d).to_s('F'), 'commission' => commission, 'commissionAsset' => asset,
      'time' => at.is_a?(Integer) ? at : ms(at), 'isBuyer' => buyer, 'isMaker' => false, 'isBestMatch' => true }
  end

  def bn_deposit(id, coin, amount, at, status: 1, tx: "0xd#{id}")
    { 'id' => id.to_s, 'amount' => amount, 'coin' => coin, 'network' => coin, 'status' => status, 'address' => 'addr', 'addressTag' => '',
      'txId' => tx, 'insertTime' => at.is_a?(Integer) ? at : ms(at), 'transferType' => 0, 'confirmTimes' => '1/1', 'unlockConfirm' => 0, 'walletType' => 0 }
  end

  def bn_withdrawal(id, coin, amount, at, fee: '0.0005', status: 6, tx: "0xw#{id}")
    { 'id' => "w#{id}", 'amount' => amount, 'transactionFee' => fee, 'coin' => coin, 'status' => status, 'address' => 'addr',
      'txId' => tx, 'applyTime' => at.utc.strftime('%Y-%m-%d %H:%M:%S'), 'network' => coin, 'transferType' => 0 }.compact
  end

  def binance_ledger(at, state) = { 'kind' => 'ledger', 'at' => at, 'binance_state' => state }

  # A file read by the tracker's import: `text` is the upload's bytes (UTF-8); `answer` names the venue of the key the
  # Binance/Binance.US question is answered with (its first key of that exchange type).
  def import(at, format, text, filename: nil, answer: nil)
    { 'kind' => 'import', 'at' => at, 'format' => format, 'filename' => filename, 'text' => text, 'answer' => answer }.compact
  end

  # Tracker::ImportsController#create's zone and chosen-key steps, then Import::Run#import!: what it answers, with the
  # accounts of an ambiguous file by id. The controller refuses a zone-less Binance name before reading a byte.
  def run_import(user_id, step)
    user = User.find(user_id)
    bytes = step['text'].b
    reader = Import::Run.reader_for(step['format'])
    offset = reader.requires_offset? ? Import::BinanceCsv.offset_from(step['filename'], bytes) : nil
    none = { 'imported' => 0, 'duplicates' => 0, 'skipped' => 0, 'unrecognised' => [], 'unrouted' => [], 'error' => nil, 'ambiguous' => nil }
    return none.merge('error' => 'unknown_timezone') if reader.requires_offset? && offset.nil?

    answered = step['answer'] && ApiKey.joins(:exchange).where(user:, exchanges: { type: step['answer'] }).order(:id).pick(:id)
    chosen = answered && ApiKey.reading(user.api_keys.includes(:exchange)).find { |key| key.id.to_s == answered.to_s }
    result = Import::Run.new(user:, format: step['format'], text: bytes, offset:, api_key: chosen).import!
    none.merge('imported' => result[:imported], 'duplicates' => result[:duplicates], 'skipped' => result[:skipped],
               'unrecognised' => result[:unrecognised], 'unrouted' => result[:unrouted], 'error' => result[:error]&.to_s,
               'ambiguous' => result[:ambiguous] ? result[:candidates].map(&:id) : nil)
  end

  BINANCE_ACCOUNT = 'GET /api/v3/account'.freeze
  EXCHANGE_RATES = 'GET /api/v1/exchange_rates'.freeze

  def binance_account(*balances, **extra)
    ok({ 'makerCommission' => 10, 'canTrade' => true, 'accountType' => 'SPOT',
         'balances' => balances.map { |asset, free, locked| { 'asset' => asset, 'free' => free, 'locked' => locked || '0.00000000' } },
         'permissions' => ['SPOT'], 'uid' => 354_937_868 }.merge(extra.transform_keys(&:to_s)))
  end

  def binance_balances(at, account:, prices: nil, rates: nil)
    { 'kind' => 'balances', 'at' => at, 'binance' => { BINANCE_ACCOUNT => [account] },
      'market' => { EXCHANGE_RATES => rates && [rates], PRICES => prices && [prices] }.compact }
  end

  def balances(at, account:, positions: ok([]), snapshots: nil, prices: nil)
    alpaca = { ACCOUNT => [account], POSITIONS => [positions], SNAPSHOTS => snapshots && [snapshots] }.compact
    { 'kind' => 'balances', 'at' => at, 'alpaca' => alpaca, 'market' => { PRICES => prices && [prices] }.compact }
  end

  NIGHT = '2026-09-20T02:00:00.250000Z'.freeze
  NEXT_NIGHT = '2026-09-21T02:00:07.500000Z'.freeze

  def filler(n, prefix: 'int', from: Time.utc(2026, 1, 1))
    Array.new(n) { |i| nta(format('%s-%04d', prefix, i), 'INT', date: (from + i.days).strftime('%Y-%m-%d'), net_amount: format('0.%02d', (i % 99) + 1)) }
  end

  def scenarios
    load File.join(__dir__, 'sync_scenarios.rb')
    SyncScenarios.all
  end

  def grid(root)
    list = scenarios
    list.each do |sc|
      dir = File.join(root, sc[:name])
      FileUtils.mkdir_p(dir)
      ctx = build(dir, sc)
      File.write(File.join(dir, 'scenario.json'),
                 JSON.pretty_generate('parity_scratch' => true, 'user_id' => ctx[:user].id, 'api_key_id' => ctx[:key].id,
                                      'credentials' => KEY, 'steps' => sc[:steps]))
      ActiveRecord::Base.connection_pool.disconnect!
    end
    puts "built #{list.size} scenarios in #{root}"
  end

  def record(root)
    ActiveJob::Base.queue_adapter = :test # the ledger walk, the broadcasts and the delayed re-bump are other plans' jobs
    Rails.cache = ActiveSupport::Cache::MemoryStore.new # production has a real cache: the venue's one-minute price cache
    Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(ScriptedSync::Adapter)
    # Outside the comparison: page broadcasts, the job's half-second pause, and the snapshot row (its own plan).
    Turbo::StreamsChannel.singleton_class.prepend(Module.new do
      %i[broadcast_remove_to broadcast_update_to broadcast_append_to broadcast_replace_to broadcast_refresh_to].each { |m| define_method(m) { |*, **| nil } }
    end)
    AccountTransaction::SyncJob.prepend(Module.new { def sleep(*) = nil })
    Exchanges::Kucoin.prepend(Module.new { def sleep(*) = nil }) # each_page's 10 s throttle wait (LEDGER_THROTTLE_WAIT)
    Exchanges::Bitvavo.prepend(Module.new { def sleep(*) = nil }) # the ledger's pacing (LEDGER_READ_PAUSE)
    Exchanges::Gemini.prepend(Module.new { def sleep(*) = nil }) # each_ledger_row's 5 s transfers pacing (TRANSFERS_REQUEST_INTERVAL)
    PortfolioSnapshot.singleton_class.prepend(Module.new { def record!(*) = nil })
    Dir[File.join(root, '*/scenario.json')].sort.each do |path|
      dir = File.dirname(path)
      sc = JSON.parse(File.read(path))
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
      Rails.cache.clear
      before = snapshot
      reading = ApiKey.reading(ApiKey.includes(:exchange)).select { |k| [Exchanges::Alpaca, Exchanges::Binance, Exchanges::Kucoin, Exchanges::Bitvavo, Exchanges::Gemini, Exchanges::Bitget].any? { |c| k.exchange.is_a?(c) } }.map(&:id).sort
      steps = sc['steps'].map do |step|
        ActiveJob::Base.queue_adapter.enqueued_jobs.clear
        ScriptedSync.alpaca = step.fetch('alpaca', {}).transform_values(&:dup)
        ScriptedSync.binance = step.fetch('binance', {}).transform_values(&:dup)
        ScriptedSync.binance_state = step['binance_state']
        ScriptedSync.kucoin = step.fetch('kucoin', {}).transform_values(&:dup)
        ScriptedSync.bitvavo = step.fetch('bitvavo', {}).transform_values(&:dup)
        ScriptedSync.gemini = step.fetch('gemini', {}).transform_values(&:dup)
        ScriptedSync.bitget = step.fetch('bitget', {}).transform_values(&:dup)
        ScriptedSync.market = step.fetch('market', {}).transform_values(&:dup)
        ScriptedSync.requests = []
        ScriptedSync.filters_after = step['server_filters_after'] == true
        raised = false
        imported = nil
        travel_to(Time.iso8601(step['at']), with_usec: true) do
          case step['kind']
          when 'ledger' then AccountTransaction::SyncJob.perform_now(ApiKey.find(sc['api_key_id']))
          when 'balances' then AccountBalance::SyncJob.perform_now(sc['user_id'], [sc['api_key_id']])
          when 'import' then imported = run_import(sc['user_id'], step)
          else raise ArgumentError, "unknown step kind #{step['kind'].inspect}"
          end
        rescue StandardError
          raised = true # the job re-raised (a transport failure Rails treats as retryable); the key's error column says what
        end
        travel_back
        # After every step, each bot's restatement_generation: a split that arrives in two syncs moves it twice.
        { 'requests' => ScriptedSync.requests, 'raised' => raised, 'generations' => Bot.order(:id).pluck(:id, :restatement_generation),
          'import' => imported }.compact
      end
      # max_nesting: a raw_data nested to the parser's limit sits several levels down in this report.
      # What Rails itself reads as a split row afterwards (Bot::Restatable#split_row?): a raw_data its JSON type cannot read is nil.
      splits_read = AccountTransaction.where(entry_type: :adjustment).order(:id).count { |row| Bot.new.send(:split_row?, row) }
      report = { 'reading' => reading, 'steps' => steps, 'splits_read' => splits_read, 'changes' => diff(before, snapshot) }
      File.write(File.join(dir, 'rails.json'), JSON.pretty_generate(report, max_nesting: false))
      ActiveRecord::Base.connection_pool.disconnect!
    end
  end
end

command, root = ARGV
raise ArgumentError, 'usage: grid <root> | record <root>' unless root

case command
when 'grid' then SyncParity.grid(root)
when 'record' then SyncParity.record(root)
else raise ArgumentError, 'usage: grid <root> | record <root>'
end
