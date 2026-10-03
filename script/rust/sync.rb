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

# A hosted instance: prices that are neither cash nor Alpaca's own come from the market-data API (MarketDataSettings).
ENV['MARKET_DATA_URL'] = 'http://data-api:3000'
ENV['MARKET_DATA_TOKEN'] = 'parity-token'

module Harness
  # An Exception, not a StandardError: with_rescue and the jobs' own rescues would turn a harness gap into a sync failure.
  class Unscripted < Exception; end # rubocop:disable Lint/InheritException
end

Net::HTTP.prepend(Module.new { def connect = raise(Harness::Unscripted, "real connection to #{address}:#{port}") })

module ScriptedSync
  mattr_accessor :alpaca, :market, :requests, :filters_after

  NETWORK = {
    'pre_send' => -> { Faraday::ConnectionFailed.new(Errno::ECONNREFUSED.new('connect(2) for "paper-api.alpaca.markets" port 443')) },
    'post_send' => -> { Faraday::TimeoutError.new(Net::ReadTimeout.new) },
    'permanent' => -> { Faraday::SSLError.new(OpenSSL::SSL::SSLError.new('certificate verify failed')) }
  }.freeze

  def self.reply(env)
    host = env.url.host.to_s
    script = if host.end_with?('alpaca.markets') then alpaca
             elsif host == 'data-api' then market
             end
    key = "#{env.method.to_s.upcase} #{env.url.path}"
    raise Harness::Unscripted, "unscripted HTTP call #{key} to #{host}" if script.nil?

    requests << [key, URI.decode_www_form(env.url.query.to_s).sort]
    queue = script[key] or raise Harness::Unscripted, "unscripted call #{key}"
    queue.size > 1 ? queue.shift : queue.first
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
        actual = "#{error.class}: #{error.message}" # what Client.network_failure reports
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

  def position(symbol, qty, asset_class: 'us_equity')
    { 'asset_id' => "asset-#{symbol}", 'symbol' => symbol, 'exchange' => asset_class == 'crypto' ? 'CRYPTO' : 'NASDAQ', 'asset_class' => asset_class,
      'asset_marginable' => asset_class != 'crypto', 'qty' => qty, 'qty_available' => qty, 'avg_entry_price' => '100', 'side' => 'long',
      'market_value' => '0', 'cost_basis' => '0', 'current_price' => '100' }
  end

  def snapshot_body(prices)
    ok(prices.transform_values { |p| p.nil? ? {} : { 'latestTrade' => { 'p' => p, 's' => 100, 't' => '2026-09-18T19:59:59.5Z', 'x' => 'V' }, 'dailyBar' => { 'c' => p } } })
  end

  def prices_body(prices) = ok('data' => prices.transform_values { |p| { 'usd' => p } })

  # ---- the install ----

  def build(dir, scenario)
    { 'production.sqlite3' => 'db/schema.rb', 'production_queue.sqlite3' => 'db/queue_schema.rb' }.each do |file, schema|
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, file))
      ActiveRecord::Schema.verbose = false
      load Rails.root.join(schema)
    end
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
    travel_to(Time.utc(2026, 9, 1, 9, 0, 0)) do
      user = User.new(name: 'Owner', email: 'owner@example.com', password: 'correct horse battery staple', admin: true,
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
      ctx = { user:, alpaca:, assets:, key: }
      scenario[:setup]&.call(ctx)
      ctx
    end
  end

  # A bot with one closed buy of `symbol`, placed at `at` (insert!: no callbacks, no jobs).
  def holder(ctx, symbol, at:, amount: '10', external_id: nil, side: 0)
    bot = ctx[:user].bots.new(type: 'Bots::DcaMultiAsset', exchange: ctx[:alpaca], settings: {
      'quote_asset_id' => ctx[:assets]['USD'].id, 'quote_amount' => 60.0, 'interval' => 'week', 'weighting' => 'manual',
      'allocations' => { ctx[:assets].fetch(symbol).id.to_s => 1.0 }
    })
    bot.set_missed_quote_amount
    bot.save!
    Transaction.insert!({ 'bot_id' => bot.id, 'exchange_id' => ctx[:alpaca].id, 'base_asset_id' => ctx[:assets].fetch(symbol).id,
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

  def balance(ctx, symbol, free:, usd_price: nil, usd_value: nil, priced_at: nil, exchange: ctx[:alpaca], user: ctx[:user])
    at = Time.utc(2026, 9, 10, 2, 30, 0)
    AccountBalance.insert!({ user_id: user.id, exchange_id: exchange.id, asset_id: ctx[:assets].fetch(symbol).id, free:, locked: 0,
                             usd_price:, usd_value:, priced_at: priced_at || (usd_price && at), synced_at: at, created_at: at, updated_at: at })
  end

  def ledger(at, *pages) = { 'kind' => 'ledger', 'at' => at, 'alpaca' => { ACTIVITIES => pages.map { |p| p.is_a?(Array) ? ok(p) : p } } }

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
    PortfolioSnapshot.singleton_class.prepend(Module.new { def record!(*) = nil })
    Dir[File.join(root, '*/scenario.json')].sort.each do |path|
      dir = File.dirname(path)
      sc = JSON.parse(File.read(path))
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
      Rails.cache.clear
      before = snapshot
      reading = ApiKey.reading(ApiKey.includes(:exchange)).select { |k| k.exchange.is_a?(Exchanges::Alpaca) }.map(&:id).sort
      steps = sc['steps'].map do |step|
        ActiveJob::Base.queue_adapter.enqueued_jobs.clear
        ScriptedSync.alpaca = step.fetch('alpaca', {}).transform_values(&:dup)
        ScriptedSync.market = step.fetch('market', {}).transform_values(&:dup)
        ScriptedSync.requests = []
        ScriptedSync.filters_after = step['server_filters_after'] == true
        raised = false
        travel_to(Time.iso8601(step['at']), with_usec: true) do
          case step['kind']
          when 'ledger' then AccountTransaction::SyncJob.perform_now(ApiKey.find(sc['api_key_id']))
          when 'balances' then AccountBalance::SyncJob.perform_now(sc['user_id'], [sc['api_key_id']])
          else raise ArgumentError, "unknown step kind #{step['kind'].inspect}"
          end
        rescue StandardError
          raised = true # the job re-raised (a transport failure Rails treats as retryable); the key's error column says what
        end
        travel_back
        # After every step, each bot's restatement_generation: a split that arrives in two syncs moves it twice.
        { 'requests' => ScriptedSync.requests, 'raised' => raised, 'generations' => Bot.order(:id).pluck(:id, :restatement_generation) }
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
