# The Rails half of the decision-parity harness (rust/tests/parity.rs, script/rust/parity_on_copy.sh).
#   bin/rails runner script/rust/decisions.rb grid <root>    # one install per scenario, built with Rails' own models
#   bin/rails runner script/rust/decisions.rb grid-alpaca <root> # the same for Alpaca, scripted beneath Clients::Alpaca at the Faraday adapter
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
  # What only the Rust engine writes into transient_data: its placement intent, and the mails it owes (rust/src/engine/notice.rs).
  MAIL_MARKERS = %w[rust_funds_mail_pending rust_error_mail_pending rust_stopped_mail_pending rust_limit_mail_pending].freeze
  RUST_KEYS = (%w[rust_placement] + MAIL_MARKERS).freeze
  # One marker of each kind, as the engine leaves them: Rails must carry them through every write of a tick, untouched.
  SEEDED_MARKERS = { 'rust_funds_mail_pending' => { 'quote_asset' => 2, 'stamped_at' => '2026-08-31T09:00:00.000Z' },
                     'rust_error_mail_pending' => { 'unknown' => { 'error' => 'an <old> "error"', 'stamped_at' => '2026-08-31T09:00:00.000Z' } },
                     'rust_stopped_mail_pending' => { 'error' => 'unauthorized.', 'stamped_at' => '2026-08-31T09:00:00.000Z' },
                     'rust_limit_mail_pending' => { 'stamped_at' => '2026-08-31T09:00:00.000Z' } }.freeze

  def raw(value) = value.is_a?(Float) ? { 'f' => [value].pack('G').unpack1('H*') } : value

  def rows(table)
    ActiveRecord::Base.connection.select_all("SELECT * FROM #{table} ORDER BY id").to_a.to_h do |r|
      r = r.transform_values { |v| raw(v) }
      JSON_COLUMNS.each { |c| r[c] = JSON.parse(r[c]) if r[c].is_a?(String) }
      r['transient_data'] = r['transient_data'].except(*RUST_KEYS) if r['transient_data'].is_a?(Hash)
      [r['id'], r]
    end
  end

  def snapshot = %w[bots transactions bot_activity_logs].to_h { |t| [t, rows(t)] }

  # The mails Rails has enqueued (Bot::Notifyable's deliver_later): the mailer action, and what it was handed.
  def enqueued_mails
    ActiveJob::Base.queue_adapter.enqueued_jobs.select { |j| j['job_class'].to_s.end_with?('MailDeliveryJob') }.map do |j|
      params = j['arguments'][3]['params']
      { 'mail' => j['arguments'][1], 'errors' => params['errors'], 'quote' => params['quote'] }.compact
    end
  end

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
    ApiKey.new(user:, exchange:, key: 'k', secret: 's', passphrase:, status: :correct, key_type: :trading).save!(validate: false)
    # As BotApi::Bots::Create does (create_basket + save_and_start), without starting a job.
    bot = user.bots.new(type: 'Bots::DcaMultiAsset', exchange:, settings: {
      'quote_asset_id' => quote.id, 'quote_amount' => sc['quote_amount'], 'interval' => sc['interval'], 'weighting' => 'manual',
      'allocations' => { btc.id.to_s => 1.0 }
    }.merge(sc['settings']))
    bot.set_missed_quote_amount
    bot.save!
    bot.update_columns(status: Bot.statuses[:scheduled], started_at: Time.iso8601(sc['started_at']),
                       settings_changed_at: sc['settings_changed_at'] && Time.iso8601(sc['settings_changed_at']),
                       transient_data: bot.reload.transient_data.merge(sc['transient']),
                       last_end_of_funds_notification: sc['funds_notified_at'] && Time.iso8601(sc['funds_notified_at']))
    sc['transactions'].each do |t|
      Transaction.insert!(t.merge('bot_id' => bot.id, 'exchange_id' => exchange.id, 'base_asset_id' => btc.id, 'quote_asset_id' => quote.id,
                                  'base' => 'BTC', 'quote' => quote.symbol, 'side' => 0, 'transaction_type' => 'REGULAR', 'bot_interval' => sc['interval'],
                                  'bot_quote_amount' => sc['quote_amount'], 'error_messages' => [], 'updated_at' => t['created_at']))
    end
    ticker.update_columns(sc['ticker_after']) if sc['ticker_after'] # e.g. delisted after the bot was set up
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
      # Listed divergences (rust/tests/parity.rs UNREADABLE): a number Rust cannot read in a price, in the placed order's
      # first poll (Kraken's AddOrder answer carries no number either side reads), and in the sweep's poll of a waiting order.
      { 'nan' => 'NaN', 'infinity' => 'Infinity', 'garbage' => 'garbage' }.each do |label, bad|
        variants["unreadable_price_#{label}"] = { 'at' => after.(1), 'http' => { '/0/public/Ticker' => [ticker_body('49990.1', bad, bad)] } }
        variants["unreadable_placed_#{label}"] = { 'at' => after.(1), 'poll' => 'OTX-1',
          'http' => ->(mode) { { '/0/private/QueryOrders' => [query_body('OTX-1' => raw_order(status: 'closed', vol: '0.0012', vol_exec: bad, cost: '59.99', price: '49991.7',
                                                                                             viqc: false, limit_price: mode == 'limit' ? '49870.3' : '0'))] } } }
        variants["unreadable_poll_#{label}"] = { 'at' => after.(1),
          'transactions' => [{ 'status' => 0, 'external_status' => 0, 'external_id' => 'OMKT-8', 'order_type' => 0, 'quote_amount' => '60',
                               'price' => '50000', 'created_at' => '2026-09-01 10:00:01' }],
          'http' => { '/0/private/QueryOrders' => [query_body('OMKT-8' => raw_order(status: 'closed', vol: '60', vol_exec: bad, cost: '60', price: '50010.5', viqc: true))] } }
      end
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
  def alpaca_order(id, status, type: 'market', notional: '60', qty: nil, filled_qty: '0', filled_avg_price: nil, limit_price: nil)
    { 'id' => id, 'client_order_id' => "rails-#{id}", 'symbol' => 'BTC/USD', 'asset_class' => 'crypto', 'notional' => notional, 'qty' => qty,
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
        # The two mail budgets, each inside its day (no mail) and a day and an hour on (a mail again).
        'funds_budget_spent' => { 'at' => after.(1), 'funds_notified_at' => (Time.iso8601(after.(1)) - 23.hours).iso8601,
                                  'http' => { 'GET /v2/account' => [account('100000', '1')] } },
        'funds_budget_reopened' => { 'at' => after.(1), 'funds_notified_at' => (Time.iso8601(after.(1)) - 25.hours).iso8601,
                                     'http' => { 'GET /v2/account' => [account('100000', '1')] } },
        'error_budget_spent' => { 'at' => after.(1), 'transient' => { 'failure_notifications' => { 'unknown' => (Time.iso8601(after.(1)) - 23.hours).iso8601 } },
                                  'http' => { 'POST /v2/orders' => [{ 'status' => 422, 'body' => { 'code' => 42_210_000, 'message' => 'qty must be > 0 & <sane>' } }] } },
        'error_budget_reopened' => { 'at' => after.(1), 'transient' => { 'failure_notifications' => { 'unknown' => (Time.iso8601(after.(1)) - 25.hours).iso8601 } },
                                     'http' => { 'POST /v2/orders' => [{ 'status' => 422, 'body' => { 'code' => 42_210_000, 'message' => 'qty must be > 0 & <sane>' } }] } },
        # The Rust engine's mail markers on the row while Rails ticks it, once into a success and once into a failure.
        'markers_survive_a_tick' => { 'at' => after.(1), 'transactions' => [closed], 'transient' => SEEDED_MARKERS },
        'markers_survive_a_failure' => { 'at' => after.(1), 'transient' => SEEDED_MARKERS, 'http' => { 'POST /v2/orders' => [{ 'status' => 403,
          'body' => { 'buying_power' => '0', 'code' => 40_310_000, 'cost_basis' => '60', 'message' => 'insufficient buying power' } }] } },
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
      # Listed divergences (rust/tests/parity.rs UNREADABLE): a number Rust cannot read in a price, in the answer to the
      # placement, and in the follow-up poll of a waiting order.
      { 'nan' => 'NaN', 'infinity' => 'Infinity', 'garbage' => 'garbage' }.each do |label, bad|
        variants["unreadable_price_#{label}"] = { 'at' => after.(1), 'http' => { 'GET /v1beta3/crypto/us/latest/quotes' => [quotes(bad)],
                                                                                 'GET /v1beta3/crypto/us/latest/trades' => [trades(bad)] } }
        variants["unreadable_placed_#{label}"] = { 'at' => after.(1),
          'http' => { 'POST /v2/orders' => [ok(alpaca_order('OTX-1', 'filled', filled_qty: bad, filled_avg_price: '64321.5'))] } }
        variants["unreadable_poll_#{label}"] = { 'at' => after.(1), 'tick' => false, 'poll' => 'OOPEN-7', 'transactions' => [waiting.('OOPEN-7', limit: true)],
          'http' => { 'GET /v2/orders/OOPEN-7' => [ok(alpaca_order('OOPEN-7', 'filled', type: 'limit', notional: nil, qty: '0.000935',
                                                                   filled_qty: bad, filled_avg_price: '64150', limit_price: '64150'))] } }
      end
      modes.flat_map do |mode, settings|
        variants.map do |name, v|
          v_http = v.fetch('http', {})
          v_http = v_http.(mode) if v_http.respond_to?(:call)
          { 'name' => "#{interval}-#{mode}-#{name}", 'venue' => 'alpaca', 'interval' => interval, 'quote_amount' => v.fetch('quote_amount', 60.0),
            'started_at' => started, 'settings' => settings, 'settings_changed_at' => v['settings_changed_at'],
            'transient' => v.fetch('transient', {}), 'transactions' => v.fetch('transactions', []), 'ticker' => ticker,
            'ticker_after' => v['ticker_after'], 'at' => v.fetch('at'), 'script' => { 'alpaca' => http.merge(v_http) },
            'tick' => v.fetch('tick', true), 'poll' => v['poll'], 'funds_notified_at' => v['funds_notified_at'] }
        end
      end
    end
  end

  def grid(root, list)
    list.each do |sc|
      dir = File.join(root, sc['name'])
      FileUtils.mkdir_p(dir)
      bot = build(dir, sc)
      File.write(File.join(dir, 'scenario.json'),
                 JSON.pretty_generate({ 'parity_scratch' => true, 'bot_id' => bot.id, 'at' => sc['at'], 'venue' => sc['venue'], 'script' => sc['script'],
                                        'tick' => sc['tick'], 'poll' => sc['poll'] }.compact))
      ActiveRecord::Base.connection_pool.disconnect!
    end
    puts "built #{list.size} scenarios in #{root}"
  end

  def retry_of(bot_id)
    ActiveJob::Base.queue_adapter.enqueued_jobs.find do |j|
      j['job_class'] == 'Bot::ActionJob' && j['executions'].to_i.positive? && j['arguments'].to_s.include?("/#{bot_id}\"")
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
      %i[broadcast_status_bar_update broadcast_new_order broadcast_updated_order broadcast_metrics_panel].each { |m| define_method(m) { |*| nil } }
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
      markers = Bot.find(sc['bot_id']).transient_data.slice(*MAIL_MARKERS)
      mails = []
      if sc.fetch('tick', true)
        travel_to(Time.iso8601(sc['at']), with_usec: true) { Bot::ActionJob.perform_now(Bot.find(sc['bot_id'])) }
        mails.concat(enqueued_mails)
        (MAX_ATTEMPTS - 1).times do
          job = retry_of(sc['bot_id']) or break
          ActiveJob::Base.queue_adapter.enqueued_jobs.clear
          travel_to(Time.at(job[:at]), with_usec: true) { ActiveJob::Base.execute(job.stringify_keys) }
          mails.concat(enqueued_mails)
        end
      end
      ActiveJob::Base.queue_adapter.enqueued_jobs.clear
      poll_error = nil
      if sc['poll'] # the follow-up poll Transaction enqueues for one order; its retries are not replayed
        order = Transaction.find_by!(bot_id: sc['bot_id'], external_id: sc['poll'])
        travel_to(Time.iso8601(sc['at']) + 5, with_usec: true) do
          Bot::FetchAndUpdateOrderJob.perform_now(order, update_missed_quote_amount: true)
        rescue StandardError => e
          # A Kraken scenario must not fail its poll, except a listed unreadable-number divergence (honeymaker's strict
          # BigDecimal() raises on "garbage"): that raise is Rails' answer, recorded for rust/tests/parity.rs.
          raise unless alpaca || File.basename(dir).include?('-unreadable_')

          poll_error = e.message # the job raised (a retry_on error is enqueued instead, and does not land here)
        end
      end
      mails.concat(enqueued_mails)
      out = { 'sent' => alpaca ? ScriptedAlpaca.sent : ScriptedKraken.sent, 'changes' => diff(before, snapshot), 'mails' => mails.sort_by { |m| m['mail'] } }
      # Only where the scenario seeded the Rust engine's markers: what Rails left of them.
      out['markers'] = Bot.find(sc['bot_id']).transient_data.slice(*MAIL_MARKERS) if markers.any?
      # Alpaca only: the funds notification (its column is excluded from the snapshot) and the follow-up's raise.
      out.merge!('funds_notified' => Bot.find(sc['bot_id']).last_end_of_funds_notification.present?, 'poll_error' => poll_error) if alpaca
      out['poll_error'] = poll_error if !alpaca && poll_error
      File.write(File.join(dir, 'rails.json'), JSON.pretty_generate(out))
      travel_back
      ActiveRecord::Base.connection_pool.disconnect!
    end
  end
end

# What the web UI does to a bot through the model, on an install whose bot carries the Rust engine's four mail markers:
# the settings form on a running bot (BotsController#update), a stop (Bots::StopsController), the settings form again on
# the stopped bot (now the interval may change). Writes <dir>/web_save.json: what was saved, and what Rails left of the markers.
def web_save(dir)
  sc = Decisions.alpaca_scenarios.find { |s| s['name'] == 'week-market-markers_survive_a_tick' }
  FileUtils.mkdir_p(dir)
  id = Decisions.build(dir, sc).id
  ActiveJob::Base.queue_adapter = :test
  Bot.prepend(Module.new do # broadcasts are UI side effects
    %i[broadcast_status_bar_update broadcast_new_order broadcast_updated_order broadcast_metrics_panel].each { |m| define_method(m) { |*| nil } }
  end)
  form = lambda do |fields|
    bot = Bot.find(id)
    bot.set_missed_quote_amount
    params = ActiveSupport::HashWithIndifferentAccess.new(fields.except(:label))
    bot.update(settings: bot.settings.merge(bot.parse_params(params).stringify_keys), label: fields[:label]) || bot.errors.full_messages
  end
  running = form.call(quote_amount: '75', limit_ordered: '1', limit_order_pcnt_distance: '0.5', label: 'Renamed in the form')
  stopped = Bot.find(id).stop(stop_message_key: 'bot.status.stopped_by_user')
  idle = form.call(quote_amount: '80', interval: 'day', label: 'Renamed again')
  after = Bot.find(id)
  File.write(File.join(dir, 'web_save.json'), JSON.pretty_generate(
    'saved_while_running' => running, 'stopped' => stopped, 'saved_while_stopped' => idle, 'status' => after.status, 'label' => after.label,
    'settings' => after.settings.slice('quote_amount', 'interval', 'limit_ordered'), 'markers' => after.transient_data.slice(*Decisions::MAIL_MARKERS)
  ))
  ActiveRecord::Base.connection_pool.disconnect!
end

command, root = ARGV
raise ArgumentError, 'usage: grid <root> | grid-alpaca <root> | record <root> | web-save <dir>' unless root

case command
when 'grid' then Decisions.grid(root, Decisions.scenarios)
when 'grid-alpaca' then Decisions.grid(root, Decisions.alpaca_scenarios)
when 'record' then Decisions.record(root)
when 'web-save' then web_save(root)
else raise ArgumentError, 'usage: grid <root> | grid-alpaca <root> | record <root> | web-save <dir>'
end
