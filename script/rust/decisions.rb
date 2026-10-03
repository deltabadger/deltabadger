# The Rails half of the decision-parity harness (rust/tests/parity.rs, script/rust/parity_on_copy.sh).
#   bin/rails runner script/rust/decisions.rb grid <root>    # one install per scenario, built with Rails' own models
#   bin/rails runner script/rust/decisions.rb grid-alpaca <root> # the same for Alpaca, scripted beneath Clients::Alpaca at the Faraday adapter
#   bin/rails runner script/rust/decisions.rb grid-basket <root>  # Alpaca crypto baskets, with the engine's recovery and the next checkpoint
#   bin/rails runner script/rust/decisions.rb grid-limit <root>   # the amount limit, its stop and its mail, on Alpaca crypto
#   bin/rails runner script/rust/decisions.rb record <root>  # Rails' ticks (and retries) per <root>/<scenario>/ -> rails.json
# Always run with every *_DATABASE_URL pointing at scratch files and PROXY_KRAKEN at a dead address. Kraken is
# scripted beneath the real client (Honeymaker::Clients::Kraken#get_public/#post_private), so every line of
# Rails' own parsing runs. Any path a scenario did not script raises Harness::Unscripted, on either venue, and so does any
# other real connection (a Net::HTTP#connect backstop beneath every client).
require 'json'
require 'fileutils'
require 'active_support/testing/time_helpers'

module Harness
  # An Exception, not a StandardError: with_rescue / honeymaker would turn a harness gap into an ordinary Failure.
  class Unscripted < Exception; end # rubocop:disable Lint/InheritException
end

# The backstop for every command (grid builds run model callbacks too): no real Net::HTTP connection, ever.
# ponytail: Net::HTTP only; httpx (eth, Hyperliquid signing) and raw sockets bypass it, and no Kraken/Alpaca tick reaches them.
Net::HTTP.prepend(Module.new { def connect = raise(Harness::Unscripted, "real connection to #{address}:#{port}") })

module ScriptedKraken
  mattr_accessor :http, :sent

  def self.reply(path, params)
    sent << params.transform_keys(&:to_s).except('nonce').compact if path == '/0/private/AddOrder'
    queue = http&.[](path) or raise Harness::Unscripted, "unscripted Kraken call #{path}"
    Result::Success.new(queue.size > 1 ? queue.shift : queue.first)
  end

  module Http
    def get_public(path, params = {}) = ScriptedKraken.reply(path, params)
    def post_private(path, body = {}) = ScriptedKraken.reply(path, body)
  end
end

# Alpaca is scripted one layer lower than Kraken: beneath Clients::Alpaca, at the Faraday adapter its connections use, so
# Rails' own middleware (json, raise_error), Clients::Alpaca#with_rescue, Client.network_failure and Exchanges::Alpaca's
# parsing all run. Only while an Alpaca scenario runs (http set); a Kraken scenario never reaches this adapter.
module ScriptedAlpaca
  mattr_accessor :http, :sent

  NETWORK = {
    'pre_send' => -> { Faraday::ConnectionFailed.new(Errno::ECONNREFUSED.new('connect(2) for "paper-api.alpaca.markets" port 443')) },
    'post_send' => -> { Faraday::TimeoutError.new(Net::ReadTimeout.new) },
    # Client.network_failure returns a Failure for an SSL cause (not retried): "Faraday::SSLError: certificate verify failed".
    'permanent' => -> { Faraday::SSLError.new(OpenSSL::SSL::SSLError.new('certificate verify failed')) }
  }.freeze

  def self.reply(env)
    raise Harness::Unscripted, "unscripted HTTP call to #{env.url}" unless env.url.host.to_s.end_with?('alpaca.markets')

    key = "#{env.method.to_s.upcase} #{env.url.path}"
    sent << JSON.parse(env.request_body).except('client_order_id') if key == 'POST /v2/orders'
    queue = http[key] or raise Harness::Unscripted, "unscripted Alpaca call #{key}"
    queue.size > 1 ? queue.shift : queue.first
  end

  module Adapter
    def call(env)
      raise Harness::Unscripted, "unscripted HTTP call #{env.method.to_s.upcase} #{env.url}" if ScriptedAlpaca.http.nil?

      reply = ScriptedAlpaca.reply(env)
      if (kind = reply['network'])
        error = ScriptedAlpaca::NETWORK.fetch(kind).call
        actual = "#{error.class}: #{error.message}" # what Client.network_failure reports
        raise Harness::Unscripted, "#{kind}: the script says #{reply['message'].inspect}, Ruby says #{actual.inspect}" unless actual == reply['message']

        raise error
      end
      body = reply['body'].is_a?(String) ? reply['body'] : JSON.generate(reply['body'])
      env.response = Faraday::Response.new
      save_response(env, reply.fetch('status', 200), body, { 'Content-Type' => 'application/json' })
      @app.call(env)
    end
  end
end

module Decisions
  extend ActiveSupport::Testing::TimeHelpers
  module_function

  JSON_COLUMNS = %w[settings transient_data details error_messages].freeze
  MAX_ATTEMPTS = 6

  def raw(value) = value.is_a?(Float) ? { 'f' => [value].pack('G').unpack1('H*') } : value

  def rows(table)
    ActiveRecord::Base.connection.select_all("SELECT * FROM #{table} ORDER BY id").to_a.to_h do |r|
      r = r.except('last_end_of_funds_notification').transform_values { |v| raw(v) }
      JSON_COLUMNS.each { |c| r[c] = JSON.parse(r[c]) if r[c].is_a?(String) }
      r['transient_data'] = r['transient_data'].except('failure_notifications', 'rust_placement', 'rust_defer_until', 'rust_amount_limit_stops_pending') if r['transient_data'].is_a?(Hash)
      [r['id'], r]
    end
  end

  # The tick rewrites a basket's members (Bot::Composition::Allocatable#update_bot_index_assets), so they are compared too.
  def snapshot = %w[bots transactions bot_activity_logs bot_index_assets].to_h { |t| [t, rows(t)] }

  def diff(before, after)
    after.to_h { |t, rows| [t, rows.filter_map { |id, row| row == before[t][id] ? nil : { 'id' => id, 'before' => before[t][id], 'after' => row } }] }
  end

  def kraken_venue(sc)
    kraken = Exchanges::Kraken.create!(name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4')
    btc = Asset.create!(external_id: 'bitcoin', symbol: 'BTC', name: 'Bitcoin', category: 'Cryptocurrency')
    eur = Asset.create!(external_id: 'EUR.FOREX', symbol: 'EUR', name: 'Euro', category: 'Currency')
    [btc, eur].each { |a| ExchangeAsset.create!(exchange: kraken, asset: a, available: true) }
    ticker = Ticker.create!(exchange: kraken, ticker: 'XBTEUR', base: 'XBT', quote: 'EUR', base_asset: btc, quote_asset: eur,
                            **sc['ticker'].symbolize_keys)
    [kraken, btc, eur, ticker, nil]
  end

  # As MarketData.sync_alpaca_crypto_listings_from_deltabadger! imports it: the pair as the ticker, quote USD.
  def alpaca_venue(sc)
    alpaca = Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25')
    btc = Asset.create!(external_id: 'bitcoin', symbol: 'BTC', name: 'Bitcoin', category: 'Cryptocurrency')
    usd = Asset.create!(external_id: 'usd', symbol: 'USD', name: 'US Dollar', category: 'Currency')
    [btc, usd].each { |a| ExchangeAsset.create!(exchange: alpaca, asset: a, available: true) }
    ticker = Ticker.create!(exchange: alpaca, ticker: 'BTC/USD', base: 'BTC', quote: 'USD', base_asset: btc, quote_asset: usd,
                            **sc['ticker'].symbolize_keys)
    [alpaca, btc, usd, ticker, 'paper']
  end

  def build(dir, sc)
    { 'production.sqlite3' => 'db/schema.rb', 'production_queue.sqlite3' => 'db/queue_schema.rb' }.each do |file, schema|
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, file))
      ActiveRecord::Schema.verbose = false
      load Rails.root.join(schema)
    end
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
    user = User.new(name: 'Owner', email: 'owner@example.com', password: 'correct horse battery staple', admin: true,
                    confirmed_at: Time.current, setup_completed: true)
    user.save!(validate: false)
    exchange, btc, quote, ticker, passphrase = sc['venue'] == 'alpaca' ? alpaca_venue(sc) : kraken_venue(sc)
    # A basket's other members, created after BTC so that a one-asset scenario's ids are what they were.
    assets = { 'BTC' => btc }
    (sc['members'] || {}).each_key { |sym| assets[sym] ||= basket_member(exchange, quote, sym) }
    ApiKey.new(user:, exchange:, key: 'k', secret: 's', passphrase:, status: :correct, key_type: :trading).save!(validate: false)
    allocations = sc['members'] ? sc['members'].to_h { |sym, w| [assets.fetch(sym).id.to_s, w] } : { btc.id.to_s => 1.0 }
    # As BotApi::Bots::Create does (create_basket + save_and_start), without starting a job; its after_save writes bot_index_assets.
    bot = user.bots.new(type: 'Bots::DcaMultiAsset', exchange:, settings: {
      'quote_asset_id' => quote.id, 'quote_amount' => sc['quote_amount'], 'interval' => sc['interval'], 'weighting' => 'manual',
      'allocations' => allocations
    }.merge(sc['settings']))
    bot.set_missed_quote_amount
    bot.save!
    bot.update_columns({ 'status' => Bot.statuses[:scheduled], 'started_at' => Time.iso8601(sc['started_at']),
                         'settings_changed_at' => sc['settings_changed_at'] && Time.iso8601(sc['settings_changed_at']),
                         'transient_data' => bot.reload.transient_data.merge(sc['transient']) }.merge(sc['bot_columns'] || {}))
    sc['transactions'].each do |t|
      asset = assets.fetch(t.fetch('asset', 'BTC'))
      Transaction.insert!(t.except('asset').merge('bot_id' => bot.id, 'exchange_id' => exchange.id, 'base_asset_id' => asset.id, 'quote_asset_id' => quote.id,
                                                  'base' => asset.symbol, 'quote' => quote.symbol, 'side' => 0, 'transaction_type' => 'REGULAR',
                                                  'bot_interval' => sc['interval'], 'bot_quote_amount' => sc['quote_amount'], 'error_messages' => [],
                                                  'updated_at' => t['created_at']))
    end
    ticker.update_columns(sc['ticker_after']) if sc['ticker_after'] # e.g. delisted after the bot was set up
    (sc['tickers_after'] || {}).each { |sym, cols| Ticker.find_by!(exchange:, base_asset: assets.fetch(sym)).update_columns(cols) }
    # Members an earlier tick exited (in_index false, still holdings), written as update_bot_index_assets writes them
    # (update_all, so updated_at stays).
    (sc['exited'] || []).each do |sym|
      bot.bot_index_assets.where(asset_id: assets.fetch(sym).id).update_all(in_index: false, exited_at: Time.iso8601(sc['started_at']) + 600)
    end
    bot
  end

  # Every field Exchanges::Kraken#get_ticker_information digs for (a, b, c, v, p, t, l, h, o), or Rails raises KeyError.
  def ticker_body(bid, ask, last) = { 'error' => [], 'result' => { 'XXBTZEUR' => {
    'a' => [ask, '1', '1.000'], 'b' => [bid, '1', '1.000'], 'c' => [last, '0.001'], 'v' => ['12.5', '30.1'], 'p' => [last, last],
    't' => [100, 250], 'l' => [bid, bid], 'h' => [ask, ask], 'o' => last } } }
  def balance_body(eur) = { 'error' => [], 'result' => { 'ZEUR' => { 'balance' => eur, 'hold_trade' => '0' } } }
  def query_body(orders) = { 'error' => [], 'result' => orders }
  def raw_order(status:, vol:, vol_exec:, cost:, price:, viqc:, limit_price: '0')
    { 'status' => status, 'vol' => vol, 'vol_exec' => vol_exec, 'cost' => cost, 'price' => price, 'oflags' => viqc ? 'viqc' : '',
      'descr' => { 'pair' => 'XBTEUR', 'type' => 'buy', 'ordertype' => limit_price == '0' ? 'market' : 'limit', 'price' => limit_price } }
  end

  def scenarios
    started = '2026-09-01T10:00:00.123456Z'
    ticker = { 'base_decimals' => 8, 'quote_decimals' => 5, 'price_decimals' => 1, 'minimum_base_size' => '0.00005', 'minimum_quote_size' => '0.5' }
    http = {
      '/0/public/Ticker' => [ticker_body('49990.1', '50000.2', '49995.3')],
      '/0/private/AddOrder' => [{ 'error' => [], 'result' => { 'txid' => ['OTX-1'] } }],
      '/0/private/BalanceEx' => [balance_body('100000')],
      '/0/private/QueryOrders' => [query_body({})],
      '/0/private/TradesHistory' => [{ 'error' => [], 'result' => { 'trades' => {}, 'count' => 0 } }]
    }
    modes = { 'market' => {}, 'limit' => { 'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.0025 },
              'smart' => { 'smart_intervaled' => true, 'smart_interval_quote_amount' => 20.0 } }
    closed = { 'status' => 0, 'external_status' => 2, 'external_id' => 'OCLOSED-1', 'order_type' => 0, 'quote_amount' => '60',
               'quote_amount_exec' => '60', 'amount_exec' => '0.0012', 'price' => '50000', 'created_at' => '2026-09-01 10:00:01' }
    %w[day week month].flat_map do |interval|
      step = { 'day' => 1.day, 'week' => 1.week, 'month' => 1.month }.fetch(interval)
      after = ->(n) { (Time.iso8601(started) + (n * step) + 1.second).iso8601(6) }
      variants = {
        'first_tick' => { 'at' => (Time.iso8601(started) + 0.5).iso8601(6) },
        'on_schedule' => { 'at' => after.(1), 'transactions' => [closed] },
        'late_3' => { 'at' => after.(3), 'transactions' => [closed] },
        'carry' => { 'at' => after.(1), 'transient' => { 'missed_quote_amount' => '12.5' } },
        'open_limit_counted' => { 'at' => after.(1),
          'transactions' => [{ 'status' => 0, 'external_status' => 1, 'external_id' => 'OOPEN-1', 'order_type' => 1, 'amount' => '0.001',
                               'price' => '50000', 'amount_exec' => '0', 'quote_amount_exec' => '0', 'created_at' => '2026-09-01 10:00:01' }],
          'http' => { '/0/private/QueryOrders' => [query_body('OOPEN-1' => raw_order(status: 'open', vol: '0.001', vol_exec: '0', cost: '0', price: '0', viqc: false, limit_price: '50000'))] } },
        # The tick's sweep of an open, unfilled limit order beside a carry stored as a JSON string. The tick's own
        # last_action_job_at write moves updated_at anyway, so an extra or missing carry write is invisible here;
        # poll_open_carry is the scenario that sees it.
        'carry_with_open_limit' => { 'at' => after.(1), 'transient' => { 'missed_quote_amount' => '25.0' },
          'transactions' => [{ 'status' => 0, 'external_status' => 1, 'external_id' => 'OOPEN-2', 'order_type' => 1, 'amount' => '0.001',
                               'price' => '50000', 'amount_exec' => '0', 'quote_amount_exec' => '0', 'created_at' => '2026-09-01 10:00:01' }],
          'http' => { '/0/private/QueryOrders' => [query_body('OOPEN-2' => raw_order(status: 'open', vol: '0.001', vol_exec: '0', cost: '0', price: '0', viqc: false, limit_price: '50000'))] } },
        'sweep_closes_with_carry' => { 'at' => after.(1), 'transient' => { 'missed_quote_amount' => '100.0' },
          'transactions' => [{ 'status' => 0, 'external_status' => 0, 'external_id' => 'OMKT-1', 'order_type' => 0, 'quote_amount' => '60',
                               'price' => '50000', 'created_at' => '2026-09-01 10:00:01' }],
          'http' => { '/0/private/QueryOrders' => [query_body('OMKT-1' => raw_order(status: 'closed', vol: '60', vol_exec: '0.00119975', cost: '60', price: '50010.5', viqc: true))] } },
        'sweep_unknown' => { 'at' => after.(1),
          'transactions' => [{ 'status' => 0, 'external_status' => 0, 'external_id' => 'OMKT-2', 'order_type' => 0, 'quote_amount' => '60',
                               'price' => '50000', 'created_at' => '2026-09-01 10:00:01' }],
          'http' => { '/0/private/QueryOrders' => [query_body('OMKT-2' => raw_order(status: 'pending', vol: '60', vol_exec: '0', cost: '0', price: '0', viqc: true))] } },
        'sweep_throttled' => { 'at' => after.(1),
          'transactions' => [{ 'status' => 0, 'external_status' => 0, 'external_id' => 'OMKT-3', 'order_type' => 0, 'quote_amount' => '60',
                               'price' => '50000', 'created_at' => '2026-09-01 10:00:01' }],
          'http' => { '/0/private/QueryOrders' => [{ 'error' => ['EAPI:Rate limit exceeded'] }] } },
        'settings_changed' => { 'at' => after.(1), 'settings_changed_at' => '2026-09-01T18:00:00Z' },
        'below_minimum' => { 'at' => after.(1), 'quote_amount' => 0.4 },
        'rejected' => { 'at' => after.(1), 'http' => { '/0/private/AddOrder' => [{ 'error' => ['EOrder:Insufficient funds'] }] } },
        # A sanctioned divergence (rust/tests/parity.rs DIVERGENCES): Rails writes a failed row, Rust keeps the intent
        # and settles it by cl_ord_id, since Kraken may have placed the order while it failed.
        'add_service_unavailable' => { 'at' => after.(1), 'http' => { '/0/private/AddOrder' => [{ 'error' => ['EService:Unavailable'] }] } },
        'rejected_throttle' => { 'at' => after.(1), 'http' => { '/0/private/AddOrder' => [{ 'error' => ['EAPI:Rate limit exceeded'] }] } },
        'blocking' => { 'at' => after.(1), 'transient' => { 'last_failure_kind' => 'invalid_key' },
                        'http' => { '/0/private/AddOrder' => [{ 'error' => ['EAPI:Invalid key'] }] } },
        'no_price' => { 'at' => after.(1), 'http' => { '/0/public/Ticker' => [{ 'error' => ['EGeneral:Internal error'] }] } },
        'low_funds' => { 'at' => after.(1), 'http' => { '/0/private/BalanceEx' => [balance_body('1')] } },
        'zero_ask' => { 'at' => after.(1), 'http' => { '/0/public/Ticker' => [ticker_body('0', '0', '0')] } },
        'fill_split_across_pages' => { 'at' => after.(1),
          'transactions' => [{ 'status' => 0, 'external_status' => 0, 'external_id' => 'OMKT-9', 'order_type' => 0, 'quote_amount' => '60',
                               'price' => '50000', 'created_at' => '2026-09-01 10:00:01' }],
          'http' => { '/0/private/TradesHistory' => [
            { 'error' => [], 'result' => { 'count' => 2, 'trades' => { 'T1' => { 'ordertxid' => 'OMKT-9', 'vol' => '0.0006', 'cost' => '30', 'fee' => '0.078', 'type' => 'buy', 'ordertype' => 'market', 'pair' => 'XXBTZEUR' } } } },
            { 'error' => [], 'result' => { 'count' => 2, 'trades' => { 'T2' => { 'ordertxid' => 'OMKT-9', 'vol' => '0.0006', 'cost' => '30.012', 'fee' => '0.078', 'type' => 'buy', 'ordertype' => 'market', 'pair' => 'XXBTZEUR' } } } }] } },
        'untradable' => { 'at' => after.(1), 'ticker_after' => { 'trading_enabled' => false } },
        # Poll-only (no tick): Bot::FetchAndUpdateOrderJob for one seeded waiting order at at + 5 s, as
        # Transaction#after_create enqueues it. Still open and unfilled, so the carry rewrite is the only write.
        'poll_open_carry' => { 'at' => after.(1), 'tick' => false, 'poll' => 'OOPEN-3', 'transient' => { 'missed_quote_amount' => '25.0' },
          'transactions' => [{ 'status' => 0, 'external_status' => 1, 'external_id' => 'OOPEN-3', 'order_type' => 1, 'amount' => '0.001',
                               'price' => '50000', 'amount_exec' => '0', 'quote_amount_exec' => '0', 'created_at' => '2026-09-01 10:00:01' }],
          'http' => { '/0/private/QueryOrders' => [query_body('OOPEN-3' => raw_order(status: 'open', vol: '0.001', vol_exec: '0', cost: '0', price: '0', viqc: false, limit_price: '50000'))] } },
        # Poll-only: the same order closes with a fill, which draws the carry down.
        'poll_closed_fill' => { 'at' => after.(1), 'tick' => false, 'poll' => 'OOPEN-4', 'transient' => { 'missed_quote_amount' => '80.0' },
          'transactions' => [{ 'status' => 0, 'external_status' => 1, 'external_id' => 'OOPEN-4', 'order_type' => 1, 'amount' => '0.001',
                               'price' => '50000', 'amount_exec' => '0', 'quote_amount_exec' => '0', 'created_at' => '2026-09-01 10:00:01' }],
          'http' => { '/0/private/QueryOrders' => [query_body('OOPEN-4' => raw_order(status: 'closed', vol: '0.001', vol_exec: '0.001', cost: '50', price: '50000', viqc: false, limit_price: '50000'))] } },
        # A tick that places OTX-1, then that order's follow-up poll at at + 5 s finds it filled.
        'tick_then_poll' => { 'at' => after.(1), 'poll' => 'OTX-1', 'transient' => { 'missed_quote_amount' => '12.5' },
          'http' => ->(mode) { { '/0/private/QueryOrders' => [query_body('OTX-1' => raw_order(status: 'closed', vol: '0.0012', vol_exec: '0.0012', cost: '59.99', price: '49991.7', viqc: false,
                                                                                             limit_price: mode == 'limit' ? '49870.3' : '0'))] } } },
        'coarse_ticker' => { 'at' => after.(1), 'ticker' => { 'base_decimals' => 0, 'quote_decimals' => 0, 'price_decimals' => 0,
                                                              'minimum_base_size' => '1', 'minimum_quote_size' => '5' },
                             'http' => { '/0/public/Ticker' => [ticker_body('9.9', '10.1', '10.0')] } }
      }
      modes.flat_map do |mode, settings|
        variants.map do |name, v|
          v_http = v.fetch('http', {})
          v_http = v_http.(mode) if v_http.respond_to?(:call)
          { 'name' => "#{interval}-#{mode}-#{name}", 'interval' => interval, 'quote_amount' => v.fetch('quote_amount', 60.0),
            'started_at' => started, 'settings' => settings, 'settings_changed_at' => v['settings_changed_at'],
            'transient' => v.fetch('transient', {}), 'transactions' => v.fetch('transactions', []), 'ticker' => v.fetch('ticker', ticker),
            'ticker_after' => v['ticker_after'], 'at' => v.fetch('at'), 'script' => { 'http' => http.merge(v_http) },
            'tick' => v.fetch('tick', true), 'poll' => v['poll'] }
        end
      end
    end
  end

  PRE_SEND = { 'network' => 'pre_send', 'message' => 'Faraday::ConnectionFailed: Connection refused - connect(2) for "paper-api.alpaca.markets" port 443' }.freeze
  POST_SEND = { 'network' => 'post_send', 'message' => 'Faraday::TimeoutError: Net::ReadTimeout' }.freeze
  CERTIFICATE = { 'network' => 'permanent', 'message' => 'Faraday::SSLError: certificate verify failed' }.freeze

  def ok(body) = { 'status' => 200, 'body' => body }
  # An order as Alpaca documents it (GET /v2/orders/{id}); only the fields Exchanges::Alpaca#parse_order_data reads matter.
  def alpaca_order(id, status, symbol: 'BTC/USD', type: 'market', notional: '60', qty: nil, filled_qty: '0', filled_avg_price: nil, limit_price: nil)
    { 'id' => id, 'client_order_id' => "rails-#{id}", 'symbol' => symbol, 'asset_class' => 'crypto', 'notional' => notional, 'qty' => qty,
      'filled_qty' => filled_qty, 'filled_avg_price' => filled_avg_price, 'order_type' => type, 'type' => type, 'side' => 'buy',
      'time_in_force' => 'gtc', 'limit_price' => limit_price, 'status' => status }
  end
  def quotes(ask) = ok('quotes' => { 'BTC/USD' => { 'ap' => ask, 'as' => 0.5, 'bp' => 64_300.25, 'bs' => 0.4, 't' => '2026-09-01T10:00:00Z' } })
  def trades(last) = ok('trades' => { 'BTC/USD' => { 'p' => last, 's' => 0.01, 't' => '2026-09-01T10:00:00Z', 'i' => 1, 'tks' => 'B' } })
  def account(cash, non_marginable = cash) = ok('id' => 'paper-account', 'status' => 'ACTIVE', 'currency' => 'USD', 'cash' => cash,
                                                'buying_power' => (cash.to_d * 2).to_s('F'), 'non_marginable_buying_power' => non_marginable)
  # Crypto positions come back compact ("BTCUSD"); #get_balances reads symbol and qty.
  def position(symbol, qty) = { 'symbol' => symbol, 'asset_class' => 'crypto', 'qty' => qty, 'qty_available' => qty, 'side' => 'long' }
  def clock(open) = ok('timestamp' => '2026-09-01T06:00:00-04:00', 'is_open' => open, 'next_open' => '2026-09-08T09:30:00-04:00',
                       'next_close' => '2026-09-01T16:00:00-04:00')

  def alpaca_scenarios
    started = '2026-09-01T10:00:00.123456Z'
    ticker = { 'base_decimals' => 9, 'quote_decimals' => 2, 'price_decimals' => 2, 'minimum_base_size' => '0.000027', 'minimum_quote_size' => '1' }
    http = {
      # Alpaca's own ask carries more decimals than the pair's price_decimals.
      'GET /v1beta3/crypto/us/latest/quotes' => [quotes(64_321.479)],
      'GET /v1beta3/crypto/us/latest/trades' => [trades(64_310.75)],
      'POST /v2/orders' => [ok(alpaca_order('OTX-1', 'pending_new'))],
      'GET /v2/account' => [account('100000')],
      'GET /v2/positions' => [ok([position('BTCUSD', '0.0015')])]
    }
    modes = { 'market' => {}, 'limit' => { 'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.0025 },
              'smart' => { 'smart_intervaled' => true, 'smart_interval_quote_amount' => 20.0 } }
    closed = { 'status' => 0, 'external_status' => 2, 'external_id' => 'OCLOSED-1', 'order_type' => 0, 'quote_amount' => '60',
               'quote_amount_exec' => '60', 'amount_exec' => '0.00093', 'price' => '64500', 'created_at' => '2026-09-01 10:00:01' }
    waiting = lambda do |ext, limit: false|
      { 'status' => 0, 'external_status' => limit ? 1 : 0, 'external_id' => ext, 'order_type' => limit ? 1 : 0,
        'quote_amount' => limit ? nil : '60', 'amount' => limit ? '0.000935' : nil, 'price' => '64150', 'amount_exec' => '0',
        'quote_amount_exec' => '0', 'created_at' => '2026-09-01 10:00:01' }.compact
    end
    %w[hour day week].flat_map do |interval|
      step = { 'hour' => 1.hour, 'day' => 1.day, 'week' => 1.week }.fetch(interval)
      after = ->(n) { (Time.iso8601(started) + (n * step) + 1.second).iso8601(6) }
      variants = {
        'first_tick' => { 'at' => (Time.iso8601(started) + 0.5).iso8601(6) },
        'on_schedule' => { 'at' => after.(1), 'transactions' => [closed] },
        'late_3' => { 'at' => after.(3), 'transactions' => [closed] },
        'carry' => { 'at' => after.(1), 'transient' => { 'missed_quote_amount' => '12.5' } },
        'settings_changed' => { 'at' => after.(1), 'settings_changed_at' => '2026-09-01T10:30:00Z' },
        'below_minimum' => { 'at' => after.(1), 'quote_amount' => 0.4 },
        'insufficient_buying_power' => { 'at' => after.(1), 'http' => { 'POST /v2/orders' => [{ 'status' => 403,
          'body' => { 'buying_power' => '0', 'code' => 40_310_000, 'cost_basis' => '60', 'message' => 'insufficient buying power' } }] } },
        'unauthorized_twice' => { 'at' => after.(1), 'transient' => { 'last_failure_kind' => 'invalid_key' },
                                  'http' => { 'POST /v2/orders' => [{ 'status' => 401, 'body' => { 'code' => 40_110_000, 'message' => 'unauthorized.' } }] } },
        # Sanctioned divergences (rust/tests/parity.rs ALPACA_DIVERGENCES): Rails writes a failed row, Rust keeps the intent.
        'add_server_error' => { 'at' => after.(1), 'http' => { 'POST /v2/orders' => [{ 'status' => 500,
                                'body' => { 'code' => 50_010_000, 'message' => 'internal server error' } }] } },
        'add_unreadable' => { 'at' => after.(1), 'http' => { 'POST /v2/orders' => [{ 'status' => 200, 'body' => 'upstream connect error' }] } },
        'add_rate_limited' => { 'at' => after.(1), 'http' => { 'POST /v2/orders' => [{ 'status' => 429,
                                'body' => { 'code' => 42_910_000, 'message' => 'rate limit exceeded' } }] } },
        'add_network_pre_send' => { 'at' => after.(1), 'http' => { 'POST /v2/orders' => [PRE_SEND] } },
        'add_network_after_send' => { 'at' => after.(1), 'http' => { 'POST /v2/orders' => [POST_SEND] } },
        # The price changes between reads; the POST fails before sending, and the retry 3 s later must reuse the cached price.
        'retry_reuses_cached_price' => { 'at' => after.(1),
          'http' => { 'GET /v1beta3/crypto/us/latest/quotes' => [quotes(64_321.479), quotes(70_000)],
                      'GET /v1beta3/crypto/us/latest/trades' => [trades(64_310.75), trades(70_000)],
                      'POST /v2/orders' => [PRE_SEND, ok(alpaca_order('OTX-1', 'pending_new'))] } },
        'zero_price' => { 'at' => after.(1), 'http' => { 'GET /v1beta3/crypto/us/latest/quotes' => [quotes(0)],
                                                         'GET /v1beta3/crypto/us/latest/trades' => [trades(0)] } },
        'missing_price' => { 'at' => after.(1), 'http' => { 'GET /v1beta3/crypto/us/latest/quotes' => [ok('quotes' => {})],
                                                            'GET /v1beta3/crypto/us/latest/trades' => [ok('trades' => {})] } },
        'sweep_partially_filled' => { 'at' => after.(1), 'transactions' => [waiting.('OOPEN-1', limit: true)],
          'http' => { 'GET /v2/orders/OOPEN-1' => [ok(alpaca_order('OOPEN-1', 'partially_filled', type: 'limit', notional: nil, qty: '0.000935',
                                                                   filled_qty: '0.0004', filled_avg_price: '64150', limit_price: '64150'))] } },
        'sweep_rejected_ignored' => { 'at' => after.(1), 'transactions' => [waiting.('OMKT-1')],
                                      'http' => { 'GET /v2/orders/OMKT-1' => [ok(alpaca_order('OMKT-1', 'rejected'))] } },
        'sweep_fills_with_carry' => { 'at' => after.(1), 'transient' => { 'missed_quote_amount' => '100.0' }, 'transactions' => [waiting.('OMKT-2')],
          'http' => { 'GET /v2/orders/OMKT-2' => [ok(alpaca_order('OMKT-2', 'filled', filled_qty: '0.000932719', filled_avg_price: '64328.1'))] } },
        'sweep_http_error' => { 'at' => after.(1), 'transactions' => [waiting.('OMKT-3')],
          'http' => { 'GET /v2/orders/OMKT-3' => [{ 'status' => 500, 'body' => { 'code' => 50_010_000, 'message' => 'internal server error' } }] } },
        # Cash and non-marginable buying power on opposite sides of the buffer: only the latter decides (#spendable_balance).
        'funds_low_buying_power' => { 'at' => after.(1), 'http' => { 'GET /v2/account' => [account('100000', '1')] } },
        'funds_low_cash_only' => { 'at' => after.(1), 'http' => { 'GET /v2/account' => [account('1', '100000')] } },
        'balance_certificate' => { 'at' => after.(1), 'http' => { 'GET /v2/account' => [CERTIFICATE] } },
        'add_certificate' => { 'at' => after.(1), 'http' => { 'POST /v2/orders' => [CERTIFICATE] } },
        # Poll-only, failing: Bot::FetchAndUpdateOrderJob raises (no row changes); both sides report the message.
        'poll_partially_filled' => { 'at' => after.(1), 'tick' => false, 'poll' => 'OOPEN-5', 'transactions' => [waiting.('OOPEN-5', limit: true)],
          'http' => { 'GET /v2/orders/OOPEN-5' => [ok(alpaca_order('OOPEN-5', 'partially_filled', type: 'limit', notional: nil, qty: '0.000935',
                                                                   filled_qty: '0.0004', filled_avg_price: '64150', limit_price: '64150'))] } },
        'poll_http_error' => { 'at' => after.(1), 'tick' => false, 'poll' => 'OOPEN-6', 'transactions' => [waiting.('OOPEN-6', limit: true)],
          'http' => { 'GET /v2/orders/OOPEN-6' => [{ 'status' => 500, 'body' => { 'code' => 50_010_000, 'message' => 'internal server error' } }] } },
        'balance_network' => { 'at' => after.(1), 'http' => { 'GET /v2/account' => [POST_SEND] } },
        'untradable' => { 'at' => after.(1), 'ticker_after' => { 'trading_enabled' => false }, 'http' => { 'GET /v2/clock' => [clock(true)] } },
        # Sanctioned divergence: Rails parks a crypto bot whose ticker went untradable behind the stock market's clock.
        'untradable_clock_closed' => { 'at' => after.(1), 'ticker_after' => { 'trading_enabled' => false }, 'http' => { 'GET /v2/clock' => [clock(false)] } },
        # Poll-only (no tick): Bot::FetchAndUpdateOrderJob for one seeded waiting order at at + 5 s.
        'poll_open_carry' => { 'at' => after.(1), 'tick' => false, 'poll' => 'OOPEN-3', 'transient' => { 'missed_quote_amount' => '25.0' },
          'transactions' => [waiting.('OOPEN-3', limit: true)],
          'http' => { 'GET /v2/orders/OOPEN-3' => [ok(alpaca_order('OOPEN-3', 'new', type: 'limit', notional: nil, qty: '0.000935', limit_price: '64150'))] } },
        'poll_closed_fill' => { 'at' => after.(1), 'tick' => false, 'poll' => 'OOPEN-4', 'transient' => { 'missed_quote_amount' => '80.0' },
          'transactions' => [waiting.('OOPEN-4', limit: true)],
          'http' => { 'GET /v2/orders/OOPEN-4' => [ok(alpaca_order('OOPEN-4', 'filled', type: 'limit', notional: nil, qty: '0.000935',
                                                                   filled_qty: '0.000935', filled_avg_price: '64150', limit_price: '64150'))] } },
        # A tick that places OTX-1; its follow-up poll at at + 5 s finds it filled (a notional market buy, or the limit).
        'tick_then_poll' => { 'at' => after.(1), 'poll' => 'OTX-1', 'transient' => { 'missed_quote_amount' => '12.5' },
          'http' => lambda do |mode|
            filled = if mode == 'limit'
                       alpaca_order('OTX-1', 'filled', type: 'limit', notional: nil, qty: '0.001130165', filled_qty: '0.001130165',
                                                       filled_avg_price: '64149.97', limit_price: '64149.97')
                     else
                       alpaca_order('OTX-1', 'filled', notional: '72.5', filled_qty: '0.001127108', filled_avg_price: '64321.5')
                     end
            { 'GET /v2/orders/OTX-1' => [ok(filled)] }
          end }
      }
      modes.flat_map do |mode, settings|
        variants.map do |name, v|
          v_http = v.fetch('http', {})
          v_http = v_http.(mode) if v_http.respond_to?(:call)
          { 'name' => "#{interval}-#{mode}-#{name}", 'venue' => 'alpaca', 'interval' => interval, 'quote_amount' => v.fetch('quote_amount', 60.0),
            'started_at' => started, 'settings' => settings, 'settings_changed_at' => v['settings_changed_at'],
            'transient' => v.fetch('transient', {}), 'transactions' => v.fetch('transactions', []), 'ticker' => ticker,
            'ticker_after' => v['ticker_after'], 'at' => v.fetch('at'), 'script' => { 'alpaca' => http.merge(v_http) },
            'tick' => v.fetch('tick', true), 'poll' => v['poll'] }
        end
      end
    end
  end

  # The basket members beside BTC/USD, with the precision data-api's listing sync gives them, and their prices.
  BASKET_PAIRS = {
    'BTC' => { 'base_decimals' => 9, 'quote_decimals' => 2, 'price_decimals' => 2, 'minimum_base_size' => '0.000027', 'minimum_quote_size' => '1' },
    'ETH' => { 'base_decimals' => 9, 'quote_decimals' => 2, 'price_decimals' => 2, 'minimum_base_size' => '0.0005', 'minimum_quote_size' => '1' },
    'SOL' => { 'base_decimals' => 9, 'quote_decimals' => 2, 'price_decimals' => 3, 'minimum_base_size' => '0.01', 'minimum_quote_size' => '1' }
  }.freeze
  BASKET_PRICES = { 'BTC' => [64_000.0, 63_990.0], 'ETH' => [2500.0, 2499.5], 'SOL' => [150.0, 149.9] }.freeze # [ask, last]
  WEIGHTS = {
    'w50' => { 'BTC' => 0.5, 'ETH' => 0.5 }, 'w70' => { 'BTC' => 0.7, 'ETH' => 0.3 },
    'thirds' => { 'BTC' => 0.334, 'ETH' => 0.333, 'SOL' => 0.333 }, 'tiers' => { 'BTC' => 0.5, 'ETH' => 0.3, 'SOL' => 0.2 }
  }.freeze
  BASKET_STARTED = '2026-09-01T10:00:00.123456Z'
  # Alpaca's own not-found envelope: the only 404 that proves an order absent.
  NOT_FOUND = { 'status' => 404, 'body' => { 'code' => 40_410_000, 'message' => 'order not found for 9b1d2c3e-0000-4000-8000-000000000001' } }.freeze
  FIVE_XX = { 'status' => 500, 'body' => { 'code' => 50_010_000, 'message' => 'internal server error' } }.freeze

  def basket_member(exchange, quote, sym)
    asset = Asset.create!(external_id: sym.downcase, symbol: sym, name: sym, category: 'Cryptocurrency')
    ExchangeAsset.create!(exchange:, asset:, available: true)
    Ticker.create!(exchange:, ticker: "#{sym}/USD", base: sym, quote: 'USD', base_asset: asset, quote_asset: quote,
                   **BASKET_PAIRS.fetch(sym).symbolize_keys)
    asset
  end

  def basket_at(days) = (Time.iso8601(BASKET_STARTED) + days.days + 1.second).iso8601(6)
  def basket_first = (Time.iso8601(BASKET_STARTED) + 0.5).iso8601(6)

  # Every member's price in one body, as Alpaca answers `symbols=…`: Exchanges::Alpaca#get_ask_price digs out its own pair.
  def basket_quotes(over = {}, without: nil)
    quotes = BASKET_PRICES.merge(over).except(without).to_h do |s, (ask, last)|
      ["#{s}/USD", { 'ap' => ask, 'as' => 0.5, 'bp' => last, 'bs' => 0.4, 't' => '2026-09-01T10:00:00Z' }]
    end
    ok('quotes' => quotes)
  end

  def basket_trades(over = {})
    ok('trades' => BASKET_PRICES.merge(over).to_h { |s, (_ask, last)| ["#{s}/USD", { 'p' => last, 's' => 0.01, 't' => '2026-09-01T10:00:00Z', 'i' => 1, 'tks' => 'B' }] })
  end

  def placed(n, sym) = ok(alpaca_order("OTX-#{n}", 'pending_new', symbol: "#{sym}/USD"))

  # A market leg filled at the ask, as a later sweep or the engine's client-order-id lookup finds it.
  def filled_leg(id, sym, notional)
    ask = BASKET_PRICES.fetch(sym)[0].to_d
    ok(alpaca_order(id, 'filled', symbol: "#{sym}/USD", notional:, filled_qty: (notional.to_d / ask).round(9).to_s('F'), filled_avg_price: ask.to_s('F')))
  end

  def basket_http
    { 'GET /v1beta3/crypto/us/latest/quotes' => [basket_quotes], 'GET /v1beta3/crypto/us/latest/trades' => [basket_trades],
      'POST /v2/orders' => [placed(1, 'BTC'), placed(2, 'ETH'), placed(3, 'SOL')], 'GET /v2/account' => [account('100000')],
      'GET /v2/positions' => [ok([])], 'GET /v2/clock' => [clock(true)] }
  end

  # One basket scenario: daily, 60 USD, started as the one-asset grids are, `members` in settings order (nil: the one-asset BTC bot).
  def basket(name, members, at:, quote_amount: 60.0, settings: {}, transient: {}, transactions: [], http: {}, **extra)
    { 'name' => name, 'venue' => 'alpaca', 'interval' => 'day', 'quote_amount' => quote_amount, 'started_at' => BASKET_STARTED,
      'settings' => settings, 'settings_changed_at' => nil, 'transient' => transient, 'transactions' => transactions,
      'ticker' => BASKET_PAIRS['BTC'], 'members' => members, 'at' => at, 'script' => { 'alpaca' => basket_http.merge(http) },
      'tick' => true }.merge(extra.transform_keys(&:to_s))
  end

  def closed_leg(n, sym, value)
    ask = BASKET_PRICES.fetch(sym)[0].to_d
    qty = (value.to_d / ask).round(9)
    { 'asset' => sym, 'status' => 0, 'external_status' => 2, 'external_id' => "OCLOSED-#{n}", 'order_type' => 0, 'quote_amount' => value.to_d.to_s('F'),
      'quote_amount_exec' => (qty * ask).to_s('F'), 'amount_exec' => qty.to_s('F'), 'price' => ask.to_s('F'), 'created_at' => '2026-09-01 10:00:01' }
  end

  # A basket's holdings before its second tick: [rows, the sweep's replies for the waiting ones].
  def basket_holdings(weights, state)
    resting = { 'asset' => 'ETH', 'status' => 0, 'external_status' => 1, 'external_id' => 'OOPEN-1', 'order_type' => 1, 'amount' => '0.0072',
                'price' => '2493.25', 'amount_exec' => '0', 'quote_amount_exec' => '0', 'created_at' => '2026-09-01 10:00:01' }
    rest = lambda do |status, filled|
      { 'GET /v2/orders/OOPEN-1' => [ok(alpaca_order('OOPEN-1', status, symbol: 'ETH/USD', type: 'limit', notional: nil, qty: '0.0072', filled_qty: filled,
                                                     filled_avg_price: filled == '0' ? nil : '2493.25', limit_price: '2493.25'))] }
    end
    case state
    when 'empty' then [[], {}]
    # The first contribution bought by weight.
    when 'at_target' then [weights.each_with_index.map { |(sym, w), i| closed_leg(i + 1, sym, (60 * w).round(2)) }, {}]
    # BTC holds the whole first contribution: its offset is zero, the others take this tick's.
    when 'drifted' then [[closed_leg(1, 'BTC', 60.0)], {}]
    # An ETH limit buy resting unfilled: its remainder counts as held (reserved_waiting_amounts) and as invested.
    when 'resting' then [[resting], rest.('new', '0')]
    # The same buy part-filled: Alpaca's partially_filled reads as unknown, so the sweep raises and the tick fails on both sides.
    when 'resting_partial' then [[resting.merge('amount_exec' => '0.003', 'quote_amount_exec' => '7.47975')], rest.('partially_filled', '0.003')]
    # A closed BTC buy that reported a zero executed quote adds no units to the ledger.
    when 'zero_quote_exec' then [[closed_leg(1, 'BTC', 42.0).merge('quote_amount_exec' => '0')], {}]
    # An ETH buy cancelled after filling 9 of 18 counts 9 as invested (pending) and 0.0036 ETH as held (ledger).
    when 'cancelled_partial'
      [[{ 'asset' => 'ETH', 'status' => 0, 'external_status' => 3, 'external_id' => 'OCAN-1', 'order_type' => 0, 'quote_amount' => '18',
          'quote_amount_exec' => '9', 'amount_exec' => '0.0036', 'price' => '2500', 'created_at' => '2026-09-01 10:00:01' }], {}]
    end
  end

  # The recovery scenarios: the one-asset bot and the 50/30/20 basket (120 USD: legs 60, 36, 24) with leg k ambiguous, the
  # engine's reconciliation 20 min + 1 s later (Rust only: Rails has no job then), and the next checkpoint (both). With
  # `limit_for`, each is the amount-limit twin: that cap, switched on at `stamp`.
  def recover_scenarios(prefix:, limit_for: nil, stamp: nil)
    recover = (Time.iso8601(basket_first) + 1201).iso8601(6)
    later = [placed(4, 'BTC'), placed(5, 'ETH'), placed(6, 'SOL')]
    swept = { 'GET /v2/orders/OTX-1' => [filled_leg('OTX-1', 'BTC', '60')], 'GET /v2/orders/OTX-2' => [filled_leg('OTX-2', 'ETH', '36')] }
    landed = filled_leg('OTX-L', 'ETH', '36').tap { |r| r['body'] = r['body'].merge('client_order_id' => '$client_order_id') }
    cases = {
      'one-network' => [nil, 60.0, [POST_SEND, placed(2, 'BTC')], NOT_FOUND],
      'one-5xx' => [nil, 60.0, [FIVE_XX, placed(2, 'BTC')], NOT_FOUND],
      'tiers-k1-network' => [WEIGHTS['tiers'], 120.0, [POST_SEND] + later, NOT_FOUND],
      'tiers-k2-network' => [WEIGHTS['tiers'], 120.0, [placed(1, 'BTC'), POST_SEND] + later, NOT_FOUND],
      'tiers-k3-network' => [WEIGHTS['tiers'], 120.0, [placed(1, 'BTC'), placed(2, 'ETH'), POST_SEND] + later, NOT_FOUND],
      'tiers-k2-5xx' => [WEIGHTS['tiers'], 120.0, [placed(1, 'BTC'), FIVE_XX] + later, NOT_FOUND],
      'tiers-k2-landed' => [WEIGHTS['tiers'], 120.0, [placed(1, 'BTC'), POST_SEND] + later, landed]
    }
    list = cases.map do |name, (members, quote_amount, posts, lookup)|
      settings = limit_for ? { 'quote_amount_limited' => true, 'quote_amount_limit' => limit_for.(members) } : {}
      transient = stamp ? { 'quote_amount_limit_enabled_at' => stamp } : {}
      basket("#{prefix}-recover-#{name}", members, at: basket_first, quote_amount:, settings:, transient:, recover_at: recover,
             next_at: basket_at(1), report_mails: limit_for ? true : nil,
             http: swept.merge('POST /v2/orders' => posts, 'GET /v2/orders:by_client_order_id' => [lookup]))
    end
    settings = limit_for ? { 'quote_amount_limited' => true, 'quote_amount_limit' => limit_for.(WEIGHTS['tiers']) } : {}
    transient = stamp ? { 'quote_amount_limit_enabled_at' => stamp } : {}
    list << landed_reference("#{prefix}-recover-tiers-k2-landed", WEIGHTS['tiers'], legs: [%w[OTX-1 BTC 60]], landed: %w[ETH 36],
                             quote_amount: 120.0, settings:, transient:, report_mails: limit_for ? true : nil)
  end

  # The Rails reference for a landed-recovery scenario `name`. The same bot at the next checkpoint
  # (`basket_at(1)`, the scenario's next_at) holding the rows the engine holds by then: each accepted leg as Rails wrote it
  # (unknown, swept to filled by this tick), and the landed leg as the engine recorded it from the client-order-id lookup
  # (the intent's quote, base and price; the lookup's fill). Rails' orders here are the ones the engine must send.
  def landed_reference(name, members, legs:, landed:, http: {}, **rest)
    first = Time.iso8601(basket_first).utc.strftime('%Y-%m-%d %H:%M:%S.%6N')
    rows = legs.map do |id, sym, q|
      { 'asset' => sym, 'status' => 0, 'external_status' => 0, 'external_id' => id, 'order_type' => 0, 'quote_amount' => q,
        'price' => BASKET_PRICES.fetch(sym)[0].to_s, 'created_at' => first }
    end
    sym, q = landed
    ask = BASKET_PRICES.fetch(sym)[0].to_d
    qty = (q.to_d / ask).round(9)
    rows << { 'asset' => sym, 'status' => 0, 'external_status' => 2, 'external_id' => 'OTX-L', 'order_type' => 0, 'quote_amount' => q,
              'amount' => (q.to_d / ask).to_s('F'), 'price' => ask.to_s('F'), 'amount_exec' => qty.to_s('F'),
              'quote_amount_exec' => (qty * ask).to_s('F'), 'created_at' => first }
    swept = legs.to_h { |id, s, v| ["GET /v2/orders/#{id}", [filled_leg(id, s, v)]] }
    basket("#{name}-reference", members, at: basket_at(1), transactions: rows,
           http: swept.merge('POST /v2/orders' => [placed(4, 'BTC'), placed(5, 'ETH'), placed(6, 'SOL')]).merge(http), **rest)
  end

  def basket_scenarios
    modes = { 'market' => {}, 'limit' => { 'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.0025 } }
    list = []
    # Sizing (26): weights × holdings × order type; the rarer holdings on the 70/30 basket only.
    %w[w50 w70 thirds].product(%w[empty at_target drifted], modes.keys).each do |w, state, mode|
      rows, http = basket_holdings(WEIGHTS[w], state)
      list << basket("basket-sizing-#{w}-#{state}-#{mode}", WEIGHTS[w], at: basket_at(1), settings: modes[mode], transactions: rows, http:)
    end
    %w[resting resting_partial zero_quote_exec cancelled_partial].product(modes.keys).each do |state, mode|
      rows, http = basket_holdings(WEIGHTS['w70'], state)
      list << basket("basket-sizing-w70-#{state}-#{mode}", WEIGHTS['w70'], at: basket_at(1), settings: modes[mode], transactions: rows, http:)
    end
    # Order safety (25): every failing outcome at every leg of the 3- and the 2-member basket; the legs before k are accepted.
    outcomes = {
      'rejected' => { 'status' => 422, 'body' => { 'code' => 42_210_000, 'message' => 'order is not allowed' } },
      'insufficient' => { 'status' => 403, 'body' => { 'buying_power' => '0', 'code' => 40_310_000, 'cost_basis' => '30', 'message' => 'insufficient buying power' } },
      'ambiguous_5xx' => FIVE_XX, 'ambiguous_network' => POST_SEND, 'pre_send' => PRE_SEND
    }
    %w[tiers w70].each do |w|
      syms = WEIGHTS[w].keys
      outcomes.each do |outcome, reply|
        (1..syms.size).each do |k|
          posts = syms.first(k - 1).each_with_index.map { |sym, i| placed(i + 1, sym) } + [reply]
          list << basket("basket-safety-#{w}-#{outcome}-k#{k}", WEIGHTS[w], at: basket_at(1), http: { 'POST /v2/orders' => posts })
        end
      end
    end
    # Minimums (4): one leg under Alpaca's 1 USD (SOL 0.96 of 4.80), and every leg under it (0.84 and 0.36 of 1.20).
    modes.each do |mode, settings|
      list << basket("basket-minimum-one_below-#{mode}", WEIGHTS['tiers'], at: basket_first, quote_amount: 4.8, settings:)
      list << basket("basket-minimum-all_below-#{mode}", WEIGHTS['w70'], at: basket_first, quote_amount: 1.2, settings:)
    end
    # Smart intervals (3): 20 USD every 8 hours of a 60 USD daily basket.
    smart = { 'smart_intervaled' => true, 'smart_interval_quote_amount' => 20.0 }
    list << basket('basket-smart-tiers-first_tick', WEIGHTS['tiers'], at: basket_first, settings: smart)
    list << basket('basket-smart-tiers-late', WEIGHTS['tiers'], at: basket_at(1), settings: smart)
    list << basket('basket-smart-w70-first_tick-limit', WEIGHTS['w70'], at: basket_first, settings: smart.merge(modes['limit']))
    # A member that stops trading at the tick (3): it is exited and the rest reweighted; or none is left.
    off = { 'trading_enabled' => false }
    list << basket('basket-untradable-one-market', WEIGHTS['w70'], at: basket_at(1), tickers_after: { 'ETH' => off })
    list << basket('basket-untradable-one-limit', WEIGHTS['w70'], at: basket_at(1), settings: modes['limit'], tickers_after: { 'ETH' => off })
    list << basket('basket-untradable-all', WEIGHTS['w70'], at: basket_at(1), tickers_after: { 'BTC' => off, 'ETH' => off })
    # Prices (2): the middle member unpriced, before any order; a limit price under SOL's 3 decimals, before any order.
    list << basket('basket-price-missing-middle', WEIGHTS['tiers'], at: basket_at(1),
                   http: { 'GET /v1beta3/crypto/us/latest/quotes' => [basket_quotes(without: 'ETH')] })
    list << basket('basket-price-zero_limit-sol', WEIGHTS['tiers'], at: basket_at(1), settings: modes['limit'],
                   http: { 'GET /v1beta3/crypto/us/latest/trades' => [basket_trades('SOL' => [150.0, 0.0004])] })
    # Carry and lifecycle (5).
    list << basket('basket-carry-w70', WEIGHTS['w70'], at: basket_at(1), transient: { 'missed_quote_amount' => '12.5' })
    list << basket('basket-late_3-tiers', WEIGHTS['tiers'], at: basket_at(3))
    list << basket('basket-settings_changed-thirds', WEIGHTS['thirds'], at: basket_at(1), settings_changed_at: '2026-09-01T18:00:00Z')
    # Yesterday's three legs, still unknown, filled by this tick's sweep before the split.
    legs = { 'OTX-A' => %w[BTC 30], 'OTX-B' => %w[ETH 18], 'OTX-C' => %w[SOL 12] }
    waiting = legs.map do |id, (sym, q)|
      { 'asset' => sym, 'status' => 0, 'external_status' => 0, 'external_id' => id, 'order_type' => 0, 'quote_amount' => q,
        'price' => BASKET_PRICES[sym][0].to_s, 'created_at' => '2026-09-01 10:00:01' }
    end
    list << basket('basket-sweep_fills-tiers', WEIGHTS['tiers'], at: basket_at(1), transactions: waiting,
                   http: legs.to_h { |id, (sym, q)| ["GET /v2/orders/#{id}", [filled_leg(id, sym, q)]] })
    list << basket('basket-tick_then_poll-w70', WEIGHTS['w70'], at: basket_at(1), poll: 'OTX-1',
                   http: { 'GET /v2/orders/OTX-1' => [filled_leg('OTX-1', 'BTC', '84')] })
    # An ambiguous leg, the engine's reconciliation, the next checkpoint (7).
    list.concat(recover_scenarios(prefix: 'basket'))
    # Exited members (2): an exited member is a holding only. SOL left on an earlier tick and holds 24 USD of units: the split
    # values BTC and ETH only (Rails' Σcurrent excludes SOL too, get_orders_data prices buyable_allocations), so both sides
    # buy 30 and 18; eligibility admits the bot (asserted on Rust's copy).
    held = [closed_leg(1, 'BTC', 30.0), closed_leg(2, 'ETH', 18.0), closed_leg(3, 'SOL', 24.0)]
    list << basket('basket-exited-holds-tiers', WEIGHTS['tiers'], at: basket_at(1), transactions: held, exited: ['SOL'],
                   tickers_after: { 'SOL' => off })
    # SOL's leg is ambiguous on the first tick; before the next, Rails' sync delists SOL and the member is exited while its
    # intent is unresolved. The engine still settles the intent by client order id (it landed) and records the fill against
    # SOL; the next checkpoint buys BTC and ETH only, and Rails, with no row for the landed 24, buys that much more.
    # The exit is stamped as `exited:` stamps it (started_at + 10 min), so the member rows compare with the reference's.
    sol_landed = filled_leg('OTX-L', 'SOL', '24').tap { |r| r['body'] = r['body'].merge('client_order_id' => '$client_order_id') }
    list << basket('basket-exited-intent-tiers-k3-landed', WEIGHTS['tiers'], at: basket_first, quote_amount: 120.0,
                   recover_at: (Time.iso8601(basket_first) + 1201).iso8601(6), next_at: basket_at(1),
                   between: ["UPDATE tickers SET trading_enabled = 0 WHERE ticker = 'SOL/USD'",
                             "UPDATE bot_index_assets SET in_index = 0, exited_at = '2026-09-01 10:10:00.123456' " \
                             "WHERE asset_id = (SELECT base_asset_id FROM tickers WHERE ticker = 'SOL/USD')"],
                   http: { 'POST /v2/orders' => [placed(1, 'BTC'), placed(2, 'ETH'), POST_SEND, placed(4, 'BTC'), placed(5, 'ETH')],
                           'GET /v2/orders/OTX-1' => [filled_leg('OTX-1', 'BTC', '60')], 'GET /v2/orders/OTX-2' => [filled_leg('OTX-2', 'ETH', '36')],
                           'GET /v2/orders:by_client_order_id' => [sol_landed] })
    list << landed_reference('basket-exited-intent-tiers-k3-landed', WEIGHTS['tiers'], legs: [%w[OTX-1 BTC 60], %w[OTX-2 ETH 36]],
                             landed: %w[SOL 24], quote_amount: 120.0, exited: ['SOL'], tickers_after: { 'SOL' => off })
  end

  LIMIT_STAMP = '2026-09-01T10:00:00.123Z' # Time#as_json of the bot's start, to the millisecond: when its limit was switched on

  def limit_row(id, ext, **cols)
    { 'status' => 0, 'external_status' => ext, 'external_id' => id, 'order_type' => 0, 'price' => '64150',
      'created_at' => '2026-09-01 10:00:01' }.merge(cols.transform_keys(&:to_s))
  end

  def limit_closed(id, value) = limit_row(id, 2, quote_amount: value, quote_amount_exec: value, amount_exec: (value.to_d / 64_150).round(9).to_s('F'))

  # One amount-limit scenario: the daily 60 USD BTC bot with `limit` switched on at its start (the stamp can be overridden).
  def limited(name, at:, limit: 100.0, settings: {}, transient: {}, **rest)
    basket("limit-#{name}", nil, at:, settings: { 'quote_amount_limited' => true, 'quote_amount_limit' => limit }.merge(settings),
           transient: { 'quote_amount_limit_enabled_at' => LIMIT_STAMP }.merge(transient), report_mails: true, **rest)
  end

  def limit_scenarios
    limit_order = { 'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.0025 }
    waiting_market = ->(id) { limit_row(id, 0, quote_amount: '60') }
    waiting_limit = ->(id) { limit_row(id, 1, order_type: 1, amount: '0.000935', amount_exec: '0', quote_amount_exec: '0') }
    resting = ->(id) { { "GET /v2/orders/#{id}" => [ok(alpaca_order(id, 'accepted'))] } }
    resting_limit = ->(id) { { "GET /v2/orders/#{id}" => [ok(alpaca_order(id, 'new', type: 'limit', notional: nil, qty: '0.000935', limit_price: '64150'))] } }
    fill = lambda do |id, status, qty, price, limit: false|
      body = limit ? alpaca_order(id, status, type: 'limit', notional: nil, qty: '0.000935', filled_qty: qty, filled_avg_price: price, limit_price: '64150')
                   : alpaca_order(id, status, filled_qty: qty, filled_avg_price: price)
      { "GET /v2/orders/#{id}" => [ok(body)] }
    end
    list = []
    # The tally (10): one row of each kind in the cap's window, at the second checkpoint (120 owed).
    tally = {
      'closed' => [[limit_closed('OC-1', '60')], {}],
      'open_limit' => [[waiting_limit.('OO-1')], resting_limit.('OO-1')],
      'unknown_market' => [[waiting_market.('OU-1')], resting.('OU-1')],
      'cancelled_partial' => [[limit_row('OX-1', 3, quote_amount: '60', quote_amount_exec: '20', amount_exec: '0.000311769')], {}],
      'cancelled_unfilled' => [[limit_row('OX-2', 3, quote_amount: '60', quote_amount_exec: '0', amount_exec: '0')], {}],
      'abandoned' => [[limit_row('OA-1', 4, quote_amount: '60')], {}],
      'failed' => [[limit_row(nil, nil, status: 1, quote_amount: '60', quote_amount_exec: '0', amount_exec: '0')], {}],
      'skipped' => [[limit_row(nil, nil, status: 2, quote_amount: '0.4', quote_amount_exec: '0', amount_exec: '0')], {}],
      'before_stamp' => [[limit_closed('OB-1', '100')], {}]
    }
    tally.each do |kind, (rows, http)|
      transient = kind == 'before_stamp' ? { 'quote_amount_limit_enabled_at' => '2026-09-01T12:00:00.000Z' } : {}
      list << limited("tally-#{kind}", at: basket_at(1), transient:, transactions: rows, http:)
    end
    list << limited('tally-closed-limit_order', at: basket_at(1), settings: limit_order, transactions: [limit_closed('OC-1', '60')])
    # The limit's states (6).
    list << limited('state-off', at: basket_at(1), limit: 50.0, settings: { 'quote_amount_limited' => false }, transactions: [limit_closed('OC-1', '60')])
    list << limited('state-unspent', at: basket_at(1), limit: 500.0)
    list << limited('state-exact', at: basket_at(1), transactions: [limit_closed('OC-1', '60'), limit_closed('OC-2', '40')])
    list << limited('state-overspent', at: basket_at(3), transactions: [limit_closed('OC-1', '60'), limit_closed('OC-2', '60')])
    list << limited('state-nil_stamp', at: basket_at(1), limit: 50.0, transient: { 'quote_amount_limit_enabled_at' => nil },
                    transactions: [limit_closed('OC-1', '60')])
    list << limited('state-under_floor', at: basket_at(1), limit: 60.005, transactions: [limit_closed('OC-1', '60')])
    # Sizing under the cap (6).
    list << limited('cut-market', at: basket_at(3), transactions: [limit_closed('OC-1', '60')])
    list << limited('cut-limit', at: basket_at(3), settings: limit_order, transactions: [limit_closed('OC-1', '60')])
    list << limited('owed_equals_available', at: basket_at(1), limit: 120.0, transactions: [limit_closed('OC-1', '60')])
    list << limited('under_minimum-market', at: basket_at(1), limit: 60.4, transactions: [limit_closed('OC-1', '60')])
    list << limited('under_minimum-limit', at: basket_at(1), limit: 60.4, settings: limit_order, transactions: [limit_closed('OC-1', '60')])
    list << limited('smart-cut', at: basket_at(1), limit: 70.0, settings: { 'smart_intervaled' => true, 'smart_interval_quote_amount' => 20.0 },
                    transactions: [limit_closed('OC-1', '60')])
    # The stop on a fill.
    stopped_bot = { 'status' => Bot.statuses[:stopped], 'stopped_at' => Time.utc(2026, 8, 1), 'stop_message_key' => 'bot.status.stopped_by_user' }
    list << limited('poll-reaches', at: basket_at(1), limit: 60.0, tick: false, poll: 'OMKT-P', transactions: [waiting_market.('OMKT-P')],
                    http: fill.('OMKT-P', 'filled', '0.0009375', '64000'))
    list << limited('poll-reaches-limit_order', at: basket_at(1), limit: 59.99, settings: limit_order, tick: false, poll: 'OLIM-P',
                    transactions: [waiting_limit.('OLIM-P')], http: fill.('OLIM-P', 'filled', '0.000935', '64150', limit: true))
    list << limited('poll-short', at: basket_at(1), tick: false, poll: 'OMKT-P', transactions: [waiting_market.('OMKT-P')],
                    http: fill.('OMKT-P', 'filled', '0.0009375', '64000'))
    list << limited('poll-stopped_bot', at: basket_at(1), limit: 60.0, tick: false, poll: 'OMKT-P', bot_columns: stopped_bot,
                    transactions: [waiting_market.('OMKT-P')], http: fill.('OMKT-P', 'filled', '0.0009375', '64000'))
    list << limited('poll-archived', at: basket_at(1), limit: 60.0, tick: false, poll: 'OMKT-P',
                    bot_columns: stopped_bot.merge('status' => Bot.statuses[:archived]),
                    transactions: [waiting_market.('OMKT-P')], http: fill.('OMKT-P', 'filled', '0.0009375', '64000'))
    list << limited('poll-cancelled_partial_reaches', at: basket_at(1), limit: 20.0, tick: false, poll: 'OMKT-C',
                    transactions: [waiting_market.('OMKT-C')], http: fill.('OMKT-C', 'canceled', '0.0003125', '64000'))
    list << limited('sweep-reaches', at: basket_at(1), limit: 60.0, transactions: [waiting_market.('OMKT-S')],
                    http: fill.('OMKT-S', 'filled', '0.0009375', '64000'))
    list << limited('tick_then_poll', at: basket_first, limit: 60.0, poll: 'OTX-1', http: { 'GET /v2/orders/OTX-1' => [filled_leg('OTX-1', 'BTC', '60')] })
    # The sweep's fill leaves 0.005, under the 0.01 floor: Rails' StopJob runs after the run, which first sizes the 0.005 (a
    # skipped row); the engine stops the bot at the end of its tick for the same reason.
    list << limited('sweep-reaches-under_floor', at: basket_at(1), limit: 60.005, transactions: [waiting_market.('OMKT-S')],
                    http: fill.('OMKT-S', 'filled', '0.0009375', '64000'))
    # Two swept fills, each finding the cap spent: two Bot::StopJobs, two `stopped` lines, two mails.
    list << limited('sweep-reaches-twice', at: basket_at(1), limit: 60.0,
                    transactions: [limit_row('OMKT-A', 0, quote_amount: '40'), limit_row('OMKT-B', 0, quote_amount: '30')],
                    http: fill.('OMKT-A', 'filled', '0.000625', '64000').merge(fill.('OMKT-B', 'filled', '0.00046875', '64000')))
    # A cancelled part-fill of 60.02 under a 60.03 cap: Rails' COALESCE bucket is a Float, so 0.0099...98 is left, under the
    # 0.01 floor, and the bot is stopped.
    list << limited('poll-cancelled_fractional_reaches', at: basket_at(1), limit: 60.03, tick: false, poll: 'OMKT-F',
                    transactions: [limit_row('OMKT-F', 0, quote_amount: '61')], http: fill.('OMKT-F', 'canceled', '0.0009378125', '64000'))
    # Ambiguous placements (3): pending; landed (Rails overspends the cap, Rust does not); proven absent.
    recover = (Time.iso8601(basket_first) + 1201).iso8601(6)
    landed = filled_leg('OTX-L', 'BTC', '60').tap { |r| r['body'] = r['body'].merge('client_order_id' => '$client_order_id') }
    list << limited('ambiguous_pending', at: basket_first, http: { 'POST /v2/orders' => [POST_SEND] })
    list << limited('ambiguous_overspend', at: basket_first, recover_at: recover, next_at: basket_at(1),
                    http: { 'POST /v2/orders' => [POST_SEND, placed(2, 'BTC')], 'GET /v2/orders:by_client_order_id' => [landed] })
    # The reference answers the next checkpoint's POST as the scenario does (OTX-2), so the two ledgers compare row for row.
    list << landed_reference('limit-ambiguous_overspend', nil, legs: [], landed: %w[BTC 60], quote_amount: 60.0,
                             http: { 'POST /v2/orders' => [placed(2, 'BTC')] },
                             settings: { 'quote_amount_limited' => true, 'quote_amount_limit' => 100.0 },
                             transient: { 'quote_amount_limit_enabled_at' => LIMIT_STAMP }, report_mails: true)
    list << limited('ambiguous_not_placed', at: basket_first, recover_at: recover, next_at: basket_at(1),
                    http: { 'POST /v2/orders' => [POST_SEND, placed(2, 'BTC')], 'GET /v2/orders:by_client_order_id' => [NOT_FOUND] })
    # The recovery scenarios under a cap.
    list.concat(recover_scenarios(prefix: 'limit', limit_for: ->(members) { members ? 200.0 : 100.0 }, stamp: LIMIT_STAMP))
  end

  def grid(root, list)
    list.each do |sc|
      dir = File.join(root, sc['name'])
      FileUtils.mkdir_p(dir)
      bot = build(dir, sc)
      File.write(File.join(dir, 'scenario.json'),
                 JSON.pretty_generate({ 'parity_scratch' => true, 'bot_id' => bot.id, 'at' => sc['at'], 'venue' => sc['venue'], 'script' => sc['script'],
                                        'tick' => sc['tick'], 'poll' => sc['poll'], 'recover_at' => sc['recover_at'], 'next_at' => sc['next_at'],
                                        'report_mails' => sc['report_mails'], 'between' => sc['between'] }.compact))
      ActiveRecord::Base.connection_pool.disconnect!
    end
    puts "built #{list.size} scenarios in #{root}"
  end

  def retry_of(bot_id)
    ActiveJob::Base.queue_adapter.enqueued_jobs.find do |j|
      j['job_class'] == 'Bot::ActionJob' && j['executions'].to_i.positive? && j['arguments'].to_s.include?("/#{bot_id}\"")
    end
  end

  # Bot::ActionJob at `at`, then the retries it enqueues for itself, as Solid Queue would run them.
  def tick_at(sc, at)
    travel_to(at, with_usec: true) { Bot::ActionJob.perform_now(Bot.find(sc['bot_id'])) }
    (MAX_ATTEMPTS - 1).times do
      job = retry_of(sc['bot_id']) or break
      ActiveJob::Base.queue_adapter.enqueued_jobs.clear
      travel_to(Time.at(job[:at]), with_usec: true) { ActiveJob::Base.execute(job.stringify_keys) }
    end
  end

  def record(root)
    ActiveJob::Base.queue_adapter = :test # jobs are Rust's to replace; only retries are replayed below
    ActiveJob::Base.retry_jitter = 0.0
    # A real cache, as production has: development's :null_store would skip Exchange#get_*_price's 5 s cache.
    Rails.cache = ActiveSupport::Cache::MemoryStore.new
    Honeymaker::Clients::Kraken.prepend(ScriptedKraken::Http)
    Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(ScriptedAlpaca::Adapter)
    Bot.prepend(Module.new do # broadcasts are UI side effects outside the comparison
      %i[broadcast_status_bar_update broadcast_new_order broadcast_updated_order broadcast_metrics_panel
         broadcast_quote_amount_limit_update broadcast_replace_to].each { |m| define_method(m) { |*, **| nil } }
    end)
    Dir[File.join(root, '*/scenario.json')].sort.each do |path|
      dir = File.dirname(path)
      sc = JSON.parse(File.read(path))
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
      Rails.cache.clear # Exchanges::Kraken caches prices by exchange/ticker id, which every scenario shares
      ActiveJob::Base.queue_adapter.enqueued_jobs.clear
      alpaca = sc['venue'] == 'alpaca'
      ScriptedKraken.http = alpaca ? {} : sc['script']['http'].transform_values(&:dup)
      ScriptedKraken.sent = []
      ScriptedAlpaca.http = alpaca ? sc['script']['alpaca'].transform_values(&:dup) : {} # {}: any Alpaca call in a Kraken scenario is unscripted
      ScriptedAlpaca.sent = []
      before = snapshot
      mails = []
      # What Transaction's after_commit enqueues for the amount limit (quote_amount_limitable.rb:94-106): Bot::StopJob runs at
      # once, at the moment of the phase that enqueued it (the engine stops in the fill's own transaction), and each mail is
      # listed, never delivered. With no limit nothing is enqueued, and the other grids are unchanged.
      settle = lambda do |time|
        jobs = ActiveJob::Base.queue_adapter.enqueued_jobs
        # deliver_later enqueues the app's ApplicationMailDeliveryJob (config/initializers/active_job.rb), not ActionMailer::MailDeliveryJob.
        mails.concat(jobs.select { |j| j['job_class'] == ActionMailer::Base.delivery_job.name }.map { |j| j['arguments'].first(2).join('#') })
        stops = jobs.select { |j| j['job_class'] == 'Bot::StopJob' }
        jobs.clear
        travel_to(time, with_usec: true) { stops.each { |j| ActiveJob::Base.execute(j.stringify_keys) } }
      end
      if sc.fetch('tick', true)
        tick_at(sc, Time.iso8601(sc['at']))
        settle.(Time.iso8601(sc['at']))
      end
      poll_error = nil
      if sc['poll'] # the follow-up poll Transaction enqueues for one order; its retries are not replayed
        order = Transaction.find_by!(bot_id: sc['bot_id'], external_id: sc['poll'])
        travel_to(Time.iso8601(sc['at']) + 5, with_usec: true) do
          Bot::FetchAndUpdateOrderJob.perform_now(order, update_missed_quote_amount: true)
        rescue StandardError => e
          raise unless alpaca # a Kraken scenario must not fail its poll

          poll_error = e.message # the job raised (a retry_on error is enqueued instead, and does not land here)
        end
        settle.(Time.iso8601(sc['at']) + 5)
      end
      # What changes between the first tick and the next, outside both engines (Rails' own sync delisting a member, the
      # member exited meanwhile). The same SQL runs on Rust's copy (parity::play).
      (sc['between'] || []).each { |sql| ActiveRecord::Base.connection.execute(sql) }
      # `recover_at` is the engine's own reconciliation tick: Rails has no job then (its next run is at
      # next_interval_checkpoint_at, action_job.rb:325-328), so nothing runs here for it. `next_at` is that next run.
      if sc['next_at']
        ActiveJob::Base.queue_adapter.enqueued_jobs.clear
        tick_at(sc, Time.iso8601(sc['next_at']))
        settle.(Time.iso8601(sc['next_at']))
      end
      out = { 'sent' => alpaca ? ScriptedAlpaca.sent : ScriptedKraken.sent, 'changes' => diff(before, snapshot) }
      out['mails'] = mails if sc['report_mails']
      # Alpaca only: the funds notification (its column is excluded from the snapshot) and the follow-up's raise.
      out.merge!('funds_notified' => Bot.find(sc['bot_id']).last_end_of_funds_notification.present?, 'poll_error' => poll_error) if alpaca
      File.write(File.join(dir, 'rails.json'), JSON.pretty_generate(out))
      travel_back
      ActiveRecord::Base.connection_pool.disconnect!
    end
  end
end

command, root = ARGV
USAGE = 'usage: grid <root> | grid-alpaca <root> | grid-basket <root> | grid-limit <root> | record <root>'.freeze
raise ArgumentError, USAGE unless root

case command
when 'grid' then Decisions.grid(root, Decisions.scenarios)
when 'grid-alpaca' then Decisions.grid(root, Decisions.alpaca_scenarios)
when 'grid-basket' then Decisions.grid(root, Decisions.basket_scenarios)
when 'grid-limit' then Decisions.grid(root, Decisions.limit_scenarios)
when 'record' then Decisions.record(root)
else raise ArgumentError, USAGE
end
