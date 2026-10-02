# The Rails half of the reference-data row parity (rust/tests/reference_parity.rs), with script/rust/decisions.rb's oracle
# discipline: one install per scenario, built by Rails itself; data-api scripted beneath Clients::MarketData at the Faraday
# adapter, so Rails' middleware (json, raise_error), Client#with_rescue, Client.network_failure and MarketData all run.
#   bin/rails runner script/rust/reference_data.rb grid <root>
#   bin/rails runner script/rust/reference_data.rb record <root>
# Run with every *_DATABASE_URL pointing at scratch files, as the Rust test's helper does.
require 'json'
require 'fileutils'
require 'active_support/testing/time_helpers'

module Harness
  # An Exception, not a StandardError: with_rescue would turn a harness gap into an ordinary Failure.
  class Unscripted < Exception; end # rubocop:disable Lint/InheritException
end

# The backstop: no real Net::HTTP connection, ever.
Net::HTTP.prepend(Module.new { def connect = raise(Harness::Unscripted, "real connection to #{address}:#{port}") })

module ScriptedDataApi
  URL = 'http://data-api:3000'.freeze # the docker alias: absolutize_logo_url rewrites it to the public host
  TOKEN = 'parity-token'.freeze
  NETWORK = {
    'pre_send' => -> { Faraday::ConnectionFailed.new(Errno::ECONNREFUSED.new('connect(2) for "data-api" port 3000')) },
    'post_send' => -> { Faraday::TimeoutError.new(Net::ReadTimeout.new) }
  }.freeze
  mattr_accessor :http, :requests

  # "GET /path?k=v&k=v", the query decoded and sorted: rust/src/venue/http.rs `request_key` builds the same.
  def self.key(env)
    query = URI.decode_www_form(env.url.query.to_s).sort.map { |k, v| "#{k}=#{v}" }.join('&')
    ["#{env.method.to_s.upcase} #{env.url.path}", query].reject(&:empty?).join('?')
  end

  module Adapter
    def call(env)
      raise Harness::Unscripted, "unscripted HTTP call #{env.url}" unless ScriptedDataApi.http && env.url.host == 'data-api'
      raise Harness::Unscripted, 'no bearer token' unless env.request_headers['Authorization'] == "Bearer #{ScriptedDataApi::TOKEN}"

      key = ScriptedDataApi.key(env)
      ScriptedDataApi.requests << key
      queue = ScriptedDataApi.http[key] || ScriptedDataApi.http[key.split('?').first] or raise Harness::Unscripted, "unscripted data-api call #{key}"
      reply = queue.size > 1 ? queue.shift : queue.first
      raise ScriptedDataApi::NETWORK.fetch(reply['network']).call if reply['network']

      body = reply['body'].is_a?(String) ? reply['body'] : JSON.generate(reply['body'])
      env.response = Faraday::Response.new
      save_response(env, reply.fetch('status', 200), body, { 'Content-Type' => 'application/json' })
      @app.call(env)
    end
  end
end

module Reference
  extend ActiveSupport::Testing::TimeHelpers
  module_function

  AT = '2026-10-02T10:31:00.123456Z'.freeze
  ON = { 'MARKET_DATA_URL' => ScriptedDataApi::URL, 'MARKET_DATA_TOKEN' => ScriptedDataApi::TOKEN }.freeze
  TABLES = %w[assets tickers exchange_assets indices bot_activity_logs app_configs].freeze
  JSON_COLUMNS = %w[top_coins top_coins_by_exchange available_exchanges weights withdrawal_chains details].freeze
  JOB_CLASSES = {
    'sync_stocks_from_deltabadger_job' => 'Asset::SyncStocksFromDeltabadgerJob',
    'sync_alpaca_crypto_from_deltabadger_job' => 'Asset::SyncAlpacaCryptoFromDeltabadgerJob',
    'sync_indices_from_coingecko_job' => 'Index::SyncFromCoingeckoJob',
    'sync_all_tickers_and_assets_job' => 'Exchange::SyncTickersAndAssetsJob',
    'fetch_all_assets_data_from_coingecko_job' => 'Asset::FetchAllAssetsDataFromCoingeckoJob',
    'prune_bot_activity_logs_job' => 'BotActivityLog::PruneJob'
  }.freeze
  PRE_SEND = { 'network' => 'pre_send', 'message' => 'Faraday::ConnectionFailed: Connection refused' }.freeze
  POST_SEND = { 'network' => 'post_send', 'message' => 'Faraday::TimeoutError: Net::ReadTimeout' }.freeze

  def raw(value) = value.is_a?(Float) ? { 'f' => [value].pack('G').unpack1('H*') } : value

  def rows(table)
    ActiveRecord::Base.connection.select_all("SELECT * FROM #{table} ORDER BY id").to_a.filter_map do |r|
      next if table == 'app_configs' && r['key'].start_with?('rust_job.')

      r = r.transform_values { |v| raw(v) }
      JSON_COLUMNS.each { |c| r[c] = JSON.parse(r[c]) if r[c].is_a?(String) }
      next [r['id'].to_s, r] unless table == 'app_configs'

      r['value'] = AppConfig.find(r['id']).value # decrypted: the ciphertext's IV is random
      # By key: an id depends on which rows each side inserted first; nothing refers to an app_configs id.
      [r['key'], r.except('id')]
    end.to_h
  end

  def snapshot = TABLES.to_h { |t| [t, rows(t)] }

  # Every row whose content changed, deletions included (after: nil), in id order.
  def diff(before, after)
    TABLES.to_h do |t|
      by_key = t == 'app_configs'
      ids = (before[t].keys | after[t].keys)
      ids = by_key ? ids.sort : ids.sort_by(&:to_i)
      [t, ids.filter_map { |id| before[t][id] == after[t][id] ? nil : { 'id' => by_key ? id : id.to_i, 'before' => before[t][id], 'after' => after[t][id] } }]
    end
  end

  # Seeds.
  def venue(type, available: true) = type.constantize.create!(name: type.demodulize, available:, maker_fee: '0.1', taker_fee: '0.1')
  def asset(external_id, symbol, category, **attrs) = Asset.create!(external_id:, symbol:, name: symbol, category:, **attrs)

  def ticker(exchange, base, quote, ticker, base_sym, quote_sym)
    Ticker.new(exchange:, base_asset: base, quote_asset: quote, ticker:, base: base_sym, quote: quote_sym, base_decimals: 8,
               quote_decimals: 2, price_decimals: 2, minimum_base_size: '0.0001', minimum_quote_size: '1').tap { |t| t.save!(validate: false) }
  end

  # Replies.
  def ok(body) = { 'status' => 200, 'body' => body }
  def error(status, body) = { 'status' => status, 'body' => body }

  def coin(id, sym, extra = {})
    { 'external_id' => id, 'symbol' => sym, 'name' => sym, 'category' => 'Cryptocurrency', 'image_url' => "https://img.example/#{id}.png",
      'color' => '#112233', 'market_cap_rank' => 10, 'market_cap' => 1_000_000, 'circulating_supply' => 1000.5,
      'url' => "https://#{id}.example" }.merge(extra)
  end

  def pair(ext, ticker, base, extra = {}, quote_ext: 'EUR.FOREX', quote: 'EUR')
    { 'ticker' => ticker, 'base' => base, 'quote' => quote, 'base_external_id' => ext, 'quote_external_id' => quote_ext,
      'base_decimals' => 8, 'quote_decimals' => 2, 'price_decimals' => 1, 'minimum_base_size' => '0.0001', 'minimum_quote_size' => '0.5',
      'maximum_base_size' => nil, 'maximum_quote_size' => '1000000' }.merge(extra)
  end

  def listing(index, extra = {})
    { 'base_asset_id' => format('crypto:coin-%02d', index), 'quote_asset_id' => 'fiat:USD', 'symbol' => format('C%02d/USD', index),
      'native_symbol' => nil, 'base_decimals' => 9, 'quote_decimals' => 2, 'price_decimals' => 2, 'minimum_base_size' => '0.000001',
      'minimum_quote_size' => '1', 'maximum_base_size' => nil, 'maximum_quote_size' => nil, 'trading_enabled' => true }.merge(extra)
  end

  def stock_listings(count)
    (1..count).map do |i|
      { 'listing_id' => i, 'base' => format('S%04d', i), 'quote' => 'USD', 'ticker' => format('S%04d', i),
        'base_external_id' => format('S%04d.US', i), 'quote_external_id' => 'USD.FOREX', 'fractionable' => true }
    end
  end

  def crypto_seed(alpaca: true, usd: true)
    @alpaca = venue('Exchanges::Alpaca') if alpaca
    @usd = asset('usd', 'USD', 'Currency') if usd
    @coins = (1..35).map { |i| asset(format('coin-%02d', i), format('C%02d', i), 'Cryptocurrency') }
    ticker(@alpaca, @coins[34], @usd, 'C35/USD', 'C35', 'USD') if alpaca && usd # not in any payload: swept
  end

  def stock_seed(legacy: false)
    @alpaca = venue('Exchanges::Alpaca')
    @usd = asset('usd', 'USD', 'Currency') # the stock sync reassigns it to Fiat
    ticker(@alpaca, asset('OLD.US', 'OLD', 'Stock'), @usd, 'OLD', 'OLD', 'USD') # not in the listings: swept
    asset('alpaca_5f1c2b', 'S0001', 'Stock') if legacy
  end

  def venue_seed
    @kraken = venue('Exchanges::Kraken')
    venue('Exchanges::Binance')
    venue('Exchanges::Gemini', available: false) # never asked
    venue('Exchanges::Alpaca')                   # a stock venue: never asked
    @btc = asset('bitcoin', 'BTC', 'Cryptocurrency')
    @eth = asset('ethereum', 'ETH', 'Cryptocurrency')
    @sol = asset('solana', 'SOL', 'Cryptocurrency')
    @eur = asset('EUR.FOREX', 'EUR', 'Currency')
    asset('usd', 'USD', 'Fiat')
  end

  def scenarios # rubocop:disable Metrics/MethodLength,Metrics/AbcSize
    list = []
    add = ->(name, job, http, env: ON, &setup) { list << { 'name' => name, 'job' => job, 'http' => http, 'env' => env, 'setup' => setup } }

    # Asset::FetchAllAssetsDataFromCoingeckoJob
    job = 'fetch_all_assets_data_from_coingecko_job'
    seed = lambda do
      asset('bitcoin', 'BTC', 'Cryptocurrency', instrument_type: 'tokenized', market_cap: 1)
      asset('pax-gold', 'PAXG', 'Cryptocurrency') # not in the feed: the registry classifies it
    end
    feed = [
      coin('bitcoin', 'BTC', 'market_cap' => 2.5e12, 'circulating_supply' => 19_876_543.123456789, 'instrument_type' => nil),
      coin('new-coin', 'NEW', 'image_url' => nil, 'logo_url' => '/logos/ab/new.png', 'market_cap_rank' => '7'),
      coin('nvda-xstock', 'NVDAX'),
      coin('ondo-thing', 'OT', 'instrument_type' => 'tokenized'),
      coin('tether-gold', 'XAUT', 'circulating_supply' => 120_456_789.12345678)
    ]
    add.('assets-import', job, { 'GET /api/v1/assets' => [ok('data' => feed)] }, &seed)
    add.('assets-empty', job, { 'GET /api/v1/assets' => [ok('data' => [])] }, &seed)
    add.('assets-server-error', job, { 'GET /api/v1/assets' => [error(500, '<html><body>oops</body></html>')] }, &seed)
    add.('assets-network', job, { 'GET /api/v1/assets' => [POST_SEND] }, &seed)
    add.('assets-duplicate', job, { 'GET /api/v1/assets' => [ok('data' => [coin('dup', 'D'), coin('dup', 'D2')])] }, &seed)
    add.('assets-unreadable', job, { 'GET /api/v1/assets' => [ok('not json')] }, &seed)

    # Exchange::SyncAllTickersAndAssetsJob (each venue's Exchange::SyncTickersAndAssetsJob)
    job = 'sync_all_tickers_and_assets_job'
    binance = [pair('bitcoin', 'BTCUSD', 'BTC', { 'minimum_base_size' => 0.00001 }, quote_ext: 'usd', quote: 'USD')]
    add.('tickers-import', job, {
      'GET /api/v1/tickers/kraken' => [ok('data' => [
        pair('bitcoin', 'XBTEUR', 'XBT', { 'minimum_base_size' => 1.0000000000000002, 'minimum_quote_size' => 0.30000000000000004 }), # BigDecimal(float): 1.0, 0.3
        pair('ethereum', 'ETHEUR', 'ETH', { 'trading_enabled' => false }),
        pair('ethereum', 'ETHEUR', 'ETH'),                     # the same pair again: deduped
        pair('solana', 'SOLEUR', 'SOL', { 'price_decimals' => nil }), # no trading params: skipped, its asset still listed
        pair('dogecoin', 'XDGEUR', 'XDG')                      # no local asset: skipped
      ])],
      'GET /api/v1/tickers/binance' => [ok('data' => binance)]
    }) do
      venue_seed
      ticker(@kraken, @btc, @eur, 'XBTEUR', 'XBT', 'EUR')
    end
    add.('tickers-reconcile', job, {
      'GET /api/v1/tickers/kraken' => [ok('data' => [pair('bitcoin', 'XBTEUR', 'XBT'), pair('ethereum', 'ETHEUR', 'ETH'), pair('solana', 'SOLEUR2', 'SOL2')])],
      'GET /api/v1/tickers/binance' => [ok('data' => [])]
    }) do
      venue_seed
      ticker(@kraken, @sol, @eur, 'XBTEUR', 'XBT', 'EUR')         # holds the symbol bitcoin needs: tombstoned, then realigned
      ticker(@kraken, @eth, @eur, 'ETHEUR-OLD', 'ETH-OLD', 'EUR') # the same pair under old names: realigned
      ticker(@kraken, @btc, @eur, 'XBTEUR-OLD', 'XBT-OLD', 'EUR')
    end
    add.('tickers-venue-fails', job, {
      'GET /api/v1/tickers/kraken' => [error(404, { 'error' => 'Invalid exchange: kraken' })],
      'GET /api/v1/tickers/binance' => [ok('data' => binance)]
    }) { venue_seed }

    # Asset::SyncAlpacaCryptoFromDeltabadgerJob
    job = 'sync_alpaca_crypto_from_deltabadger_job'
    key = 'GET /api/v2/listings?venue=alpaca_crypto'
    healthy = (1..31).map { |i| listing(i, i == 2 ? { 'trading_enabled' => false } : {}) }
    healthy[2] = healthy[2].except('trading_enabled')          # absent: enabled
    healthy[3] = healthy[3].merge('native_symbol' => 'C04USD') # the venue's own symbol is the ticker
    healthy += [listing(1, 'base_asset_id' => 'crypto:unknown', 'symbol' => 'UNK/USD'), listing(1, 'symbol' => 'BAD')]
    add.('crypto-healthy', job, { key => [ok('data' => healthy)] }) { crypto_seed }
    add.('crypto-creates-usd-degraded', job, { key => [ok('data' => (1..29).map { |i| listing(i) })] }) { crypto_seed(usd: false) }
    add.('crypto-degraded-baseline', job, { key => [ok('data' => (1..35).map { |i| listing(i) })] }) do
      crypto_seed
      AppConfig.set('alpaca_crypto_listings_last_good_count', '40')
    end
    add.('crypto-baseline-unchanged', job, { key => [ok('data' => healthy)] }) do
      crypto_seed
      AppConfig.set('alpaca_crypto_listings_last_good_count', '31')
    end
    add.('crypto-rate-limited', job, { key => [error(429, { 'error' => 'rate limited' })] }) { crypto_seed }
    add.('crypto-network', job, { key => [PRE_SEND] }) { crypto_seed }
    add.('crypto-server-error', job, { key => [error(500, { 'error' => 'boom' })] }) { crypto_seed }
    add.('crypto-no-alpaca', job, { key => [ok('data' => healthy)] }) { crypto_seed(alpaca: false) }

    # Asset::SyncStocksFromDeltabadgerJob
    job = 'sync_stocks_from_deltabadger_job'
    stocks = (1..1005).map do |i|
      { 'asset_id' => "stock:S#{i}", 'external_id' => format('S%04d.US', i), 'type' => (i % 7).zero? ? 'etf' : 'stock',
        'symbol' => format('S%04d', i), 'name' => "Company #{i}", 'market_cap_rank' => i, 'image_url' => i.even? ? "https://img.example/s#{i}.png" : nil,
        'color' => '#445566', 'logo_url' => "/logos/s/#{i}.png", 'identifiers' => [{ 'scheme' => 'alpaca', 'value' => format('us_equity:S%04d', i) }] }
    end
    stocks << { 'external_id' => 'F0001.US', 'type' => 'fund', 'symbol' => 'F1', 'name' => 'Fund' } # an unknown type: skipped
    listings = stock_listings(1003) + [
      stock_listings(1004).last.merge('fractionable' => false), # dropped
      stock_listings(1).first.merge('ticker' => 'NOPE', 'base' => 'NOPE', 'base_external_id' => 'NOPE.US') # unresolved
    ]
    assets_key = 'GET /api/v2/assets?include=identifiers&type=stock,etf'
    listings_key = 'GET /api/v2/listings?venue_scheme=alpaca_exchange'
    both = { assets_key => [ok('data' => stocks)], listings_key => [ok('data' => listings)] }
    add.('stocks-healthy', job, both) { stock_seed }
    add.('stocks-off-switch', job, {}) do
      stock_seed
      AppConfig.set('stock_sync_enabled', 'false')
    end
    add.('stocks-legacy-row', job, both) { stock_seed(legacy: true) }
    add.('stocks-assets-fail', job, { assets_key => [error(500, { 'error' => 'boom' })] }) { stock_seed }
    add.('stocks-listings-rate-limited', job, { assets_key => [ok('data' => stocks)], listings_key => [error(429, { 'error' => 'slow' })] }) { stock_seed }
    add.('stocks-degraded', job, { assets_key => [ok('data' => stocks)], listings_key => [ok('data' => stock_listings(999))] }) { stock_seed }
    add.('stocks-degraded-baseline', job, both) do
      stock_seed
      AppConfig.set('alpaca_listings_last_good_count', '2000')
    end
    add.('stocks-no-provider', job, {}, env: {}) { stock_seed }

    # Index::SyncFromCoingeckoJob (the deltabadger branch)
    job = 'sync_indices_from_coingecko_job'
    seed = -> { Index.create!(external_id: 'layer-1', source: 'coingecko', name: 'L1', weight: 12, top_coins: ['a']) }
    indices = [
      { 'external_id' => 'nasdaq-100', 'source' => 'deltabadger', 'name' => 'Nasdaq 100', 'description' => 'The 100 largest',
        'top_coins' => %w[AAPL.US MSFT.US], 'top_coins_by_exchange' => nil, 'available_exchanges' => { 'Exchanges::Alpaca' => 100 },
        'market_cap' => 2.5e13, 'weight' => 0, 'weights' => { 'AAPL.US' => 0.0812, 'MSFT.US' => 0.0799 } },
      { 'external_id' => 'layer-1', 'source' => 'coingecko', 'name' => 'Layer 1', 'description' => 'L1s', 'top_coins' => nil, 'market_cap' => '123.5' },
      { 'external_id' => 'meme-token', 'source' => 'coingecko', 'name' => 'Meme', 'weight' => nil, 'market_cap' => 42 },
      { 'external_id' => 'other', 'source' => 'coingecko', 'name' => 'Other', 'market_cap' => nil }
    ]
    add.('indices-import', job, { 'GET /api/v2/indices' => [ok('data' => indices)] }, &seed)
    add.('indices-empty', job, { 'GET /api/v2/indices' => [ok('data' => [])] }, &seed)
    add.('indices-server-error', job, { 'GET /api/v2/indices' => [error(500, { 'error' => 'boom' })] }, &seed)

    # BotActivityLog::PruneJob: 90 days before AT is 2026-07-04 10:31:00.123456; exactly that old stays.
    add.('prune', 'prune_bot_activity_logs_job', {}) do
      ActiveRecord::Base.connection.disable_referential_integrity do
        %w[2026-07-04T10:31:00.123455Z 2026-07-04T10:31:00.123456Z 2026-07-05T00:00:00Z].each do |t|
          BotActivityLog.insert!({ bot_id: 1, event: 'x', level: 0, details: {}, created_at: Time.iso8601(t) })
        end
      end
    end
    list
  end

  def build(dir, scenario)
    { 'production.sqlite3' => 'db/schema.rb', 'production_queue.sqlite3' => 'db/queue_schema.rb' }.each do |file, schema|
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, file))
      ActiveRecord::Schema.verbose = false
      load Rails.root.join(schema)
    end
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
    travel_to(Time.iso8601('2026-09-01T00:00:00Z')) { scenario['setup']&.call }
  end

  def grid(root)
    list = scenarios
    list.each do |sc|
      dir = File.join(root, sc['name'])
      FileUtils.mkdir_p(dir)
      build(dir, sc)
      enc = ActiveRecord::Encryption.config
      File.write(File.join(dir, 'scenario.json'), JSON.pretty_generate({
        'parity_scratch' => true, 'job' => sc['job'], 'at' => AT, 'env' => sc['env'], 'script' => sc['http'],
        # The keys this Rails encrypted the scenario's app_configs with, so the Rust side reads them as Rails does.
        'encryption' => { 'primary_key' => Array(enc.primary_key).first, 'key_derivation_salt' => enc.key_derivation_salt }
      }))
      ActiveRecord::Base.connection_pool.disconnect!
    end
    puts "built #{list.size} scenarios in #{root}"
  end

  # SyncAllTickersAndAssetsJob only enqueues one job per venue, a minute apart: each runs now, in id order, and fails alone.
  def perform(job)
    if job == 'sync_all_tickers_and_assets_job'
      Exchange.available.where.not(type: Exchange::STOCK_TYPES).order(:id).each do |exchange|
        Exchange::SyncTickersAndAssetsJob.perform_now(exchange)
      rescue StandardError
        nil
      end
    else
      JOB_CLASSES.fetch(job).constantize.perform_now
    end
  end

  def record(root)
    ActiveJob::Base.queue_adapter = :test # a retry is recorded, never run
    Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(ScriptedDataApi::Adapter)
    Dir[File.join(root, '*/scenario.json')].sort.each do |path|
      dir = File.dirname(path)
      sc = JSON.parse(File.read(path))
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
      ActiveJob::Base.queue_adapter.enqueued_jobs.clear
      %w[MARKET_DATA_URL MARKET_DATA_TOKEN].each { |k| sc['env'].key?(k) ? ENV[k] = sc['env'][k] : ENV.delete(k) }
      ScriptedDataApi.http = sc['script'].transform_values(&:dup)
      ScriptedDataApi.requests = []
      before = snapshot
      travel_to(Time.iso8601(sc['at']), with_usec: true) { perform(sc['job']) }
      retried = ActiveJob::Base.queue_adapter.enqueued_jobs.any? do |j|
        j['job_class'] == JOB_CLASSES.fetch(sc['job']) && j['executions'].to_i.positive?
      end
      File.write(File.join(dir, 'rails.json'), JSON.pretty_generate({ 'requests' => ScriptedDataApi.requests, 'retry' => retried,
                                                                     'changes' => diff(before, snapshot) }))
      travel_back
      ActiveRecord::Base.connection_pool.disconnect!
    end
  end
end

command, root = ARGV
raise ArgumentError, 'usage: grid <root> | record <root>' unless root && %w[grid record].include?(command)

Reference.public_send(command, root)
