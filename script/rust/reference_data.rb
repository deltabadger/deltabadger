# The Rails half of the reference-data row parity (rust/tests/reference_parity.rs), with script/rust/decisions.rb's oracle
# discipline: one install per scenario, built by Rails itself; data-api scripted beneath Clients::MarketData at the Faraday
# adapter, so Rails' middleware (json, raise_error), Client#with_rescue, Client.network_failure and MarketData all run.
#   bin/rails runner script/rust/reference_data.rb grid <root>
#   bin/rails runner script/rust/reference_data.rb record <root>
# Run with every *_DATABASE_URL pointing at scratch files, as the Rust test's helper does.
require 'json'
require 'fileutils'
require 'active_support/testing/time_helpers'
require_relative 'schema_template'

module Harness
  # An Exception, not a StandardError: with_rescue would turn a harness gap into an ordinary Failure.
  class Unscripted < Exception; end # rubocop:disable Lint/InheritException
end

# The backstop: no real Net::HTTP connection, ever.
Net::HTTP.prepend(Module.new { def connect = raise(Harness::Unscripted, "real connection to #{address}:#{port}") })

module ScriptedDataApi
  URL = 'http://data-api:3000'.freeze # the docker alias: absolutize_logo_url rewrites it to the public host
  TOKEN = 'parity-token'.freeze
  # PLAN-A17: CoinGecko is scripted beneath the same adapters. Its requests are logged with their host (the plan ping
  # picks pro or demo), and answered by the host-less key, as rust/src/venue/http.rs's ScriptedTransport answers them.
  COINGECKO_KEY = 'CG-parity-key'.freeze
  COINGECKO_HOSTS = { 'pro-api.coingecko.com' => 'x-cg-pro-api-key', 'api.coingecko.com' => 'x-cg-demo-api-key' }.freeze
  # PLAN-A17 A17-2: the venues' public catalogues, scripted by "METHOD host/path?query" (each venue's host apart, so two
  # venues sharing a path never share an answer). Unsigned: no key is configured.
  VENUE_HOSTS = %w[api.kraken.com api.coinbase.com].freeze
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
      host = env.url.host
      known = host == 'data-api' || ScriptedDataApi::COINGECKO_HOSTS.key?(host) || ScriptedDataApi::VENUE_HOSTS.include?(host)
      raise Harness::Unscripted, "unscripted HTTP call #{env.url}" unless ScriptedDataApi.http && known
      if host == 'data-api'
        raise Harness::Unscripted, 'no bearer token' unless env.request_headers['Authorization'] == "Bearer #{ScriptedDataApi::TOKEN}"
      elsif ScriptedDataApi::COINGECKO_HOSTS.key?(host)
        header = ScriptedDataApi::COINGECKO_HOSTS.fetch(host)
        raise Harness::Unscripted, "no #{header}" unless env.request_headers[header] == ScriptedDataApi::COINGECKO_KEY
      end

      key = ScriptedDataApi.key(env)
      hosted = host == 'data-api' ? key : key.sub(' ', " #{host}")
      ScriptedDataApi.requests << hosted
      queue = [hosted, hosted.split('?').first, key, key.split('?').first].lazy.filter_map { |k| ScriptedDataApi.http[k] }.first
      raise Harness::Unscripted, "unscripted data-api call #{hosted}" unless queue

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
    'fetch_data_from_coingecko_job' => 'Asset::FetchDataFromCoingeckoJob',
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
    @owner = @key = nil
    @alpaca = venue('Exchanges::Alpaca')
    @usd = asset('usd', 'USD', 'Currency') # the stock sync reassigns it to Fiat
    ticker(@alpaca, asset('OLD.US', 'OLD', 'Stock'), @usd, 'OLD', 'OLD', 'USD') # not in the listings: swept
    asset('alpaca_5f1c2b', 'S0001', 'Stock') if legacy
  end

  # A scheduled DCA bot on `asset`, with the Alpaca paper key the engine needs to run it.
  def running_bot(asset)
    @owner ||= User.new(name: 'Owner', email: 'owner@example.com', password: 'correct horse battery staple', admin: true,
                        confirmed_at: Time.current, setup_completed: true).tap { |u| u.save!(validate: false) }
    @key ||= ApiKey.new(user: @owner, exchange: @alpaca, key: 'k', secret: 's', passphrase: 'paper', status: :correct,
                        key_type: :trading).tap { |k| k.save!(validate: false) }
    bot = @owner.bots.new(type: 'Bots::DcaMultiAsset', exchange: @alpaca, settings: {
      'quote_asset_id' => @usd.id, 'quote_amount' => 60, 'interval' => 'week', 'weighting' => 'manual', 'allocations' => { asset.id.to_s => 1.0 }
    })
    bot.set_missed_quote_amount
    bot.save!(validate: false)
    bot.update_columns(status: Bot.statuses[:scheduled], started_at: Time.iso8601('2026-08-30T00:00:00Z'), settings_changed_at: nil)
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

  # A CoinGecko install (PLAN-A17): the provider and the key as the settings page saves them, no data-api.
  def coingecko_install(key: ScriptedDataApi::COINGECKO_KEY)
    AppConfig.market_data_provider = MarketDataSettings::PROVIDER_COINGECKO
    AppConfig.coingecko_api_key = key
  end

  # A coins/markets row as CoinGecko answers it.
  def gecko(id, sym, extra = {})
    { 'id' => id, 'symbol' => sym, 'name' => sym.upcase, 'image' => "https://coin-images.coingecko.com/coins/images/1/large/#{id}.png",
      'current_price' => 1.5, 'market_cap' => 123_456_789.0, 'market_cap_rank' => 42, 'circulating_supply' => 1_000_000.25 }.merge(extra)
  end

  def markets_key(ids) = "GET /api/v3/coins/markets?ids=#{ids.join(',')}&locale=en&order=market_cap_desc&page=1&per_page=250&precision=full&sparkline=false&vs_currency=usd"

  def coingecko_scenarios(add)
    off = {}
    demo = { 'GET /api/v3/ping' => [error(401, { 'error' => 'invalid key' })] }
    pro = { 'GET /api/v3/ping' => [ok('gecko_says' => '(V3) To the Moon!')] }
    seed = lambda do
      coingecko_install
      asset('bitcoin', 'BTC', 'Cryptocurrency', name: 'BTC', url: 'https://www.coingecko.com/coins/bitcoin',
            image_url: 'https://coin-images.coingecko.com/coins/images/1/large/bitcoin.png', market_cap: 123_456_789, market_cap_rank: 42,
            circulating_supply: '1000000.25') # unchanged by the answer: update! writes nothing
      asset('ethereum', 'ETH', 'Cryptocurrency')
      asset('pax-gold', 'PAXG', 'Cryptocurrency') # absent from the answer; the registry classifies it
      asset('covalent', 'CQT', 'Cryptocurrency')  # blacklisted: asked, never written
      asset('AAPL.US', 'AAPL', 'Stock')           # not crypto: not asked
      asset('usd', 'USD', 'Fiat')
      asset('nameless', 'NL', 'Cryptocurrency')   # answered without a name, and last: Rails raises there (D-CG-ASSET-ROW-SKIPS)
    end
    ids = %w[bitcoin ethereum pax-gold covalent nameless]
    answer = [gecko('bitcoin', 'btc'), gecko('ethereum', 'eth', 'market_cap' => 4.2e11, 'circulating_supply' => 120_000_000.123456789,
                                                   'market_cap_rank' => 2, 'image' => nil),
              gecko('covalent', 'cqt'), gecko('nameless', 'nl', 'name' => nil), gecko('bitcoin', 'dup-ignored')]
    job = 'fetch_all_assets_data_from_coingecko_job'
    add.('coingecko-assets', job, demo.merge(markets_key(ids) => [ok(answer)]), env: off, &seed)
    add.('coingecko-assets-pro', job, pro.merge(markets_key(ids) => [ok(answer)]), env: off, &seed)
    add.('coingecko-assets-429', job, demo.merge(markets_key(ids) => [error(429, { 'status' => { 'error_code' => 429 } })]), env: off, &seed)
    add.('coingecko-assets-invalid-key', job, demo.merge(markets_key(ids) => [error(401, { 'status' => { 'error_code' => 10_002 } })]), env: off, &seed)
    # A ping 429 is D-CG-PLAN-PING (Rails falls to demo and goes on asking): Rust-only, tests/reference_jobs.rs.
    add.('coingecko-assets-blank-key', job, {}, env: off) do
      seed.call
      AppConfig.coingecko_api_key = ''
    end
    many = (1..251).map { |i| format('coin-%03d', i) }
    add.('coingecko-assets-two-batches', job, demo.merge(markets_key(many.first(250)) => [ok([gecko('coin-001', 'c1')])],
                                                         markets_key(many.last(1)) => [ok([gecko('coin-251', 'c251')])]), env: off) do
      coingecko_install
      many.each { |id| asset(id, id.upcase, 'Cryptocurrency') }
    end
    one = 'fetch_data_from_coingecko_job'
    coin_key = 'GET /api/v3/coins/ethereum?community_data=false&developer_data=false&localization=false&market_data=true&sparkline=false&tickers=false'
    detail = { 'id' => 'ethereum', 'symbol' => 'eth', 'name' => 'Ethereum', 'web_slug' => 'ethereum-web',
               'image' => { 'thumb' => 't.png', 'large' => 'https://coin-images.coingecko.com/coins/images/279/large/ethereum.png' },
               'market_cap_rank' => 2, 'market_data' => { 'market_cap' => { 'usd' => 4.2e11 }, 'circulating_supply' => 120_000_000.5 } }
    add.('coingecko-asset-one', one, demo.merge(coin_key => [ok(detail)]), env: off, asset: 'ethereum', &seed)
    add.('coingecko-asset-one-blacklisted', one, {}, env: off, asset: 'covalent', &seed)
    add.('coingecko-asset-one-failed', one, demo.merge(coin_key => [error(404, { 'error' => 'coin not found' })]), env: off, asset: 'ethereum', raises: true, &seed)
  end

  # PLAN-A17 A17-2: the venue tickers on a CoinGecko install, Rails' real Exchange::Synchronizer over scripted venue
  # catalogues (Kraken's AssetPairs, Coinbase's public products, shaped as the venues document them) and scripted
  # CoinGecko crawl pages. Rows (tickers, exchange_assets, assets) and requests are compared.
  KRAKEN_PAIRS = 'GET api.kraken.com/0/public/AssetPairs?aclass_base=all'.freeze
  COINBASE_PRODUCTS = 'GET api.coinbase.com/api/v3/brokerage/market/products'.freeze

  def kraken_pair(base, quote, extra = {})
    ["X#{base}Z#{quote}", { 'altname' => "#{base}#{quote}", 'wsname' => "#{base}/#{quote}", 'aclass_base' => 'currency', 'base' => base,
                            'quote' => quote, 'lot_decimals' => 8, 'cost_decimals' => 5, 'pair_decimals' => 1, 'ordermin' => '0.0001',
                            'costmin' => '0.45', 'status' => 'online' }.merge(extra)]
  end

  def kraken(*pairs) = { KRAKEN_PAIRS => [ok('error' => [], 'result' => pairs.to_h)] }

  def coinbase_product(base, quote, extra = {})
    { 'product_id' => "#{base}-#{quote}", 'base_increment' => '0.00000001', 'quote_increment' => '0.01', 'price_increment' => '0.001',
      'base_min_size' => '0.00000001', 'quote_min_size' => '1', 'base_max_size' => '3400', 'quote_max_size' => '150000000', 'status' => 'online',
      'trading_disabled' => false }.merge(extra)
  end

  def coinbase(*products) = { COINBASE_PRODUCTS => [ok('products' => products)] }
  def crawl_key(id, page) = "GET /api/v3/exchanges/#{id}/tickers?depth=false&include_exchange_logo=false&order=base_asset&page=#{page}"
  def cg_ticker(base, coin, target, target_coin = nil) = { 'base' => base, 'target' => target, 'coin_id' => coin, 'target_coin_id' => target_coin }

  def crawl(id, *pages)
    pages.each_with_index.to_h { |page, i| [crawl_key(id, i + 1), [page.is_a?(Array) ? ok('tickers' => page) : page]] }
  end

  # A ticker with the exchange assets Rails' validation needs to update it.
  def listed_ticker(exchange, base, quote, name, base_sym, quote_sym)
    [base, quote].each { |a| ExchangeAsset.find_or_create_by!(exchange:, asset: a) }
    ticker(exchange, base, quote, name, base_sym, quote_sym)
  end

  def cg_venues(bitmart: false)
    coingecko_install
    @kraken = venue('Exchanges::Kraken')
    @coinbase = venue('Exchanges::Coinbase')
    venue('Exchanges::Bitmart') if bitmart # retired: Rails fails it without a request (D-CG-RETIRED-NOT-ASKED)
    venue('Exchanges::Alpaca') # a stock venue: never asked
    @btc = asset('bitcoin', 'BTC', 'Cryptocurrency')
    @usd = asset('USD.FOREX', 'USD', 'Currency')
  end

  def coingecko_ticker_scenarios(add) # rubocop:disable Metrics/MethodLength,Metrics/AbcSize
    job = 'sync_all_tickers_and_assets_job'
    off = {}
    demo = { 'GET /api/v3/ping' => [error(401, { 'error' => 'invalid key' })] }
    full = (1..100).map { |i| cg_ticker(format('F%03d', i), format('filler-%03d', i), 'USD') }

    # New pairs resolved (a fiat quote minted from the crawl), existing ones updated in place, an unresolved one waiting, the
    # retired venue not asked.
    add.('cg-tickers-import', job, demo
      .merge(kraken(kraken_pair('XBT', 'USD', 'ordermin' => '0.00005'), kraken_pair('ETH', 'EUR', 'lot_decimals' => 6),
                    kraken_pair('SOL', 'USD', 'status' => 'limit_only'), kraken_pair('DOT', 'USD')))
      .merge(crawl('kraken', [cg_ticker('ETH', 'ethereum', 'EUR'), cg_ticker('SOL', 'solana', 'USD'), cg_ticker('BTC', 'bitcoin', 'USD')]))
      .merge(coinbase(coinbase_product('BTC', 'USD', 'quote_min_size' => '2'), coinbase_product('ETH', 'USD', 'limit_only' => true)))
      .merge(crawl('gdax', [cg_ticker('ETH', 'ethereum', 'USD')])), env: off) do
      cg_venues(bitmart: true)
      listed_ticker(@kraken, @btc, @usd, 'XBTUSD', 'XBT', 'USD')
      listed_ticker(@coinbase, @btc, @usd, 'BTC-USD', 'BTC', 'USD')
    end
    # Coinbase RNDR -> RENDER: the blacklist drops RNDR before the collision; the ticker takes the new spelling in place.
    rndr = [cg_ticker('RNDR', 'render-token', 'USD'), cg_ticker('RENDER', 'render-token', 'USD')]
    add.('cg-tickers-rename', job, demo.merge(kraken).merge(coinbase(coinbase_product('RNDR', 'USD'), coinbase_product('RENDER', 'USD'),
                                                                     coinbase_product('BTC', 'USD'))).merge(crawl('gdax', rndr)), env: off) do
      cg_venues
      listed_ticker(@coinbase, asset('render-token', 'RENDER', 'Cryptocurrency'), @usd, 'RNDR-USD', 'RNDR', 'USD')
      listed_ticker(@coinbase, @btc, @usd, 'BTC-USD', 'BTC', 'USD')
    end
    # The rename is the only overlap: the guard keeps the sweep off.
    add.('cg-tickers-rename-only', job, demo.merge(kraken).merge(coinbase(coinbase_product('RENDER', 'USD'))).merge(crawl('gdax', rndr)), env: off) do
      cg_venues
      listed_ticker(@coinbase, asset('render-token', 'RENDER', 'Cryptocurrency'), @usd, 'RNDR-USD', 'RNDR', 'USD')
      listed_ticker(@coinbase, @btc, @usd, 'BTC-USD', 'BTC', 'USD').update_columns(available: false)
    end
    # Both spellings live: the new one is refused by the (exchange, base asset, quote asset) uniqueness; the old one stays.
    add.('cg-tickers-both-spellings', job, demo.merge(kraken(kraken_pair('FOO', 'USD'), kraken_pair('FOO2', 'USD')))
      .merge(crawl('kraken', [cg_ticker('FOO2', 'foo-coin', 'USD')])).merge(coinbase), env: off) do
      cg_venues
      listed_ticker(@kraken, asset('foo-coin', 'FOO', 'Cryptocurrency'), @usd, 'FOOUSD', 'FOO', 'USD')
    end
    # The sweep guard both ways: Kraken's empty catalogue sweeps nothing; Coinbase's confirms one and sweeps the other with
    # its exchange asset; a catalogue with no overlap (its one pair unresolved) sweeps nothing either.
    add.('cg-tickers-sweep', job, demo.merge(kraken).merge(coinbase(coinbase_product('BTC', 'USD'))), env: off) do
      cg_venues
      listed_ticker(@kraken, @btc, @usd, 'XBTUSD', 'XBT', 'USD')
      listed_ticker(@coinbase, @btc, @usd, 'BTC-USD', 'BTC', 'USD')
      listed_ticker(@coinbase, asset('ethereum', 'ETH', 'Cryptocurrency'), @usd, 'ETH-USD', 'ETH', 'USD')
    end
    add.('cg-tickers-sweep-no-overlap', job, demo.merge(kraken(kraken_pair('NEW', 'USD')))
      .merge(crawl('kraken', [cg_ticker('OTHER', 'other-coin', 'USD')])).merge(coinbase), env: off) do
      cg_venues
      listed_ticker(@kraken, @btc, @usd, 'XBTUSD', 'XBT', 'USD')
    end
    # A catalogue failure fails that venue only.
    add.('cg-tickers-catalogue-fails', job, demo.merge(KRAKEN_PAIRS => [error(503, { 'error' => ['EService:Unavailable'] })])
      .merge(coinbase(coinbase_product('BTC', 'USD'), coinbase_product('ETH', 'USD'))).merge(crawl('gdax', [cg_ticker('ETH', 'ethereum', 'USD')])), env: off) do
      cg_venues
      listed_ticker(@kraken, @btc, @usd, 'XBTUSD', 'XBT', 'USD')
      listed_ticker(@coinbase, @btc, @usd, 'BTC-USD', 'BTC', 'USD')
    end
    # A crawl failing on page 2 after a full page 1, and 50 full pages: discovery discarded, the catalogue applied.
    pairs = kraken(kraken_pair('XBT', 'USD', 'pair_decimals' => 2), kraken_pair('ETH', 'USD'))
    add.('cg-tickers-crawl-page-2-fails', job, demo.merge(pairs).merge(crawl('kraken', full, error(500, { 'error' => 'boom' })))
      .merge(coinbase), env: off) do
      cg_venues
      listed_ticker(@kraken, @btc, @usd, 'XBTUSD', 'XBT', 'USD')
    end
    add.('cg-tickers-crawl-50-pages', job, demo.merge(pairs).merge(crawl('kraken', *Array.new(50, full))).merge(coinbase), env: off) do
      cg_venues
      listed_ticker(@kraken, @btc, @usd, 'XBTUSD', 'XBT', 'USD')
    end
    # The scheduled job with a blank key fails each venue before its catalogue (sync_tickers_from_coingecko!).
    add.('cg-tickers-blank-key', job, {}, env: off) do
      cg_venues
      listed_ticker(@kraken, @btc, @usd, 'XBTUSD', 'XBT', 'USD')
      AppConfig.coingecko_api_key = ''
    end
    # Kraken: DOGE becomes XDG in XDG's place (CoinGecko's own XDG overwritten) and COPM is blacklisted, both before the
    # collision check, so OMI keeps ecomi and nothing is ambiguous.
    add.('cg-tickers-kraken-xdg', job, demo.merge(kraken(kraken_pair('XDG', 'USD'), kraken_pair('OMI', 'USD'), kraken_pair('COPM', 'USD')))
      .merge(crawl('kraken', [cg_ticker('XDG', 'xdg-other', 'USD'), cg_ticker('DOGE', 'dogecoin', 'USD'), cg_ticker('COPM', 'ecomi', 'USD'),
                              cg_ticker('OMI', 'ecomi', 'USD')])).merge(coinbase), env: off) { cg_venues }
    # D-CG-SYMBOL-AMBIGUOUS: SYM has two ids and shared-id two symbols. Rails binds SYM to the first id and ONE to shared-id
    # (first wins); Rust leaves SYM, ONE and TWO unresolved and mints neither coin. The ambiguous rows come last in Rails'
    # order, so every other row matches id for id.
    add.('cg-tickers-ambiguous', job, demo.merge(kraken(kraken_pair('AAA', 'USD'), kraken_pair('SYM', 'USD'), kraken_pair('ONE', 'USD'), kraken_pair('TWO', 'USD')))
      .merge(crawl('kraken', [cg_ticker('AAA', 'good-a', 'USD'), cg_ticker('SYM', 'sym-one', 'USD'), cg_ticker('SYM', 'sym-two', 'USD'),
                              cg_ticker('ONE', 'shared-id', 'USD'), cg_ticker('TWO', 'shared-id', 'USD')])).merge(coinbase), env: off) { cg_venues }
    # D-CG-429-NO-MORE-CRAWLS: Kraken's crawl answers 429; Rails crawls Coinbase next, Rust does not (its catalogue still
    # applies). Coinbase's discovery rows come last, so the rest matches id for id.
    add.('cg-tickers-429', job, demo.merge(kraken(kraken_pair('XBT', 'USD', 'lot_decimals' => 7), kraken_pair('NEW', 'USD')))
      .merge(crawl('kraken', error(429, { 'status' => { 'error_code' => 429 } })))
      .merge(coinbase(coinbase_product('BTC', 'USD', 'quote_min_size' => '3'), coinbase_product('ETH', 'USD')))
      .merge(crawl('gdax', [cg_ticker('ETH', 'ethereum', 'USD')])), env: off) do
      cg_venues
      listed_ticker(@kraken, @btc, @usd, 'XBTUSD', 'XBT', 'USD')
      listed_ticker(@coinbase, @btc, @usd, 'BTC-USD', 'BTC', 'USD')
    end
  end

  # PLAN-A17 A17-4: Index::SyncFromCoingeckoJob's coingecko branch over scripted `coins/categories` and
  # `coins/markets?category=` answers. AT's day (2026-10-02, JDN mod 14 = 4) pins the slice: payments, music and pump-fun are
  # due (MD5(id) mod 14 = 4); meme-token (13) and storage (12) are not. Rust ranks Top Coins first (D-INDEX-TOP-N): its
  # `coins/markets` page is answered empty here, which Rust refuses and writes nothing for; Rails never asks it.
  TOP_COINS_KEY = 'GET /api/v3/coins/markets?locale=en&order=market_cap_desc&page=1&per_page=250&precision=full&sparkline=false&vs_currency=usd'.freeze
  CATEGORIES_KEY = 'GET /api/v3/coins/categories?order=market_cap_desc'.freeze

  def category_key(id) = "GET /api/v3/coins/markets?category=#{id}&locale=en&order=market_cap_desc&page=1&per_page=250&precision=full&sparkline=false&vs_currency=usd"
  def gecko_category(id, name, content) = { 'id' => id, 'name' => name, 'content' => content, 'market_cap' => 1.5e9, 'market_cap_change_24h' => 1.25 }
  def category_members(*ids) = ids.each_with_index.map { |id, i| gecko(id, id, 'market_cap' => 9e9 - (i * 1e8)) }

  # Kraken lists coin-01..11 at EUR; Binance none (an empty per-venue list); coin-12 and the unlisted coins have no venue.
  # `settled`: every row's availability already what a refresh derives, for the runs where Rails returns before refreshing.
  def cg_category_seed(settled: false)
    coingecko_install
    kraken = venue('Exchanges::Kraken')
    venue('Exchanges::Binance')
    eur = asset('EUR.FOREX', 'EUR', 'Currency')
    coins = (1..12).map { |i| asset(format('coin-%02d', i), format('C%02d', i), 'Cryptocurrency') }
    coins.first(11).each { |c| ticker(kraken, c, eur, "#{c.symbol}EUR", c.symbol, 'EUR') }
    %w[u1 u2 u3].each { |id| asset("unlisted-#{id}", id.upcase, 'Cryptocurrency') }
    three = %w[coin-01 coin-02 coin-03]
    kraken3 = settled ? { 'Exchanges::Kraken' => 3 } : {}
    row = ->(id, **attrs) { Index.create!({ external_id: id, source: 'coingecko', name: id, description: id, weight: 0, top_coins_by_exchange: {}, available_exchanges: {} }.merge(attrs)) }
    row.('old-category', top_coins: %w[coin-01])                                # gone from the feed: swept
    row.('data-style', top_coins: three, weights: three.to_h { |c| [c, 0.1] },  # data-api's shape under source coingecko: swept
                       top_coins_by_exchange: { 'Exchanges::Kraken' => three }, available_exchanges: { 'Exchanges::Kraken' => 3 })
    row.('meme-token', top_coins: three, available_exchanges: kraken3)          # not due: only its availability refreshed
    row.('music', top_coins: %w[coin-01 coin-02])                               # due, two valid members: disqualified
    row.('pump-fun', top_coins: %w[unlisted-u1 unlisted-u2 unlisted-u3])        # due, no venue: skipped, kept
  end

  def coingecko_category_scenarios(add) # rubocop:disable Metrics/AbcSize
    job = 'sync_indices_from_coingecko_job'
    off = {}
    base = { 'GET /api/v3/ping' => [error(401, { 'error' => 'invalid key' })], TOP_COINS_KEY => [ok([])] }
    feed = [gecko_category('stablecoins', 'Stablecoins', 'Pegged'), gecko_category('payments', 'Payments (PAY)', 'Payment coins'),
            gecko_category('music', 'Music', 'Music coins'), gecko_category('pump-fun', 'Pump.fun Ecosystem', 'Launched on pump.fun'),
            gecko_category('launchpad', 'Launchpad', ''), gecko_category('meme-token', 'Meme (MEME)', 'Memes')]
    twelve = (1..12).map { |i| format('coin-%02d', i) }
    slice = { CATEGORIES_KEY => [ok(feed)],
              category_key('payments') => [ok(category_members(*twelve, 'not-an-asset', 'coin-01'))], # 12 valid: Rails stores 10
              category_key('music') => [ok(category_members('coin-01', 'coin-02', 'not-an-asset'))],
              category_key('pump-fun') => [ok(category_members('unlisted-u1', 'unlisted-u2', 'unlisted-u3'))] }
    add.('coingecko-categories', job, base.merge(slice), env: off) { cg_category_seed }
    # Nothing eligible: Rails' empty guard sweeps nothing.
    add.('coingecko-categories-empty', job, base.merge(CATEGORIES_KEY => [ok([gecko_category('stablecoins', 'S', 'Pegged'), gecko_category('payments', 'P', '')])]),
         env: off) { cg_category_seed(settled: true) }
    # D-CG-INDEX-PULL-RETRIED: a failed list writes nothing; Rails returns, Rust's run is Transient (retried).
    add.('coingecko-categories-failed', job, base.merge(CATEGORIES_KEY => [error(500, { 'error' => 'boom' })]), env: off) { cg_category_seed(settled: true) }
    # D-CG-INDEX-429-STOPS: music answers 429; Rails asks pump-fun next (skipped: no venue), Rust asks nothing more.
    add.('coingecko-categories-429-midway', job, base.merge(slice).merge(category_key('music') => [error(429, { 'status' => { 'error_code' => 429 } })]),
         env: off) { cg_category_seed }
    # The MD5/JDN rule: of two eligible categories only the bucketed one is asked.
    add.('coingecko-categories-slice-day', job, base.merge(CATEGORIES_KEY => [ok([gecko_category('storage', 'Storage', 'Files'), gecko_category('payments', 'Payments', 'Pay')])],
                                                          category_key('payments') => [ok(category_members('coin-01', 'coin-02', 'coin-03'))]),
         env: off) { cg_category_seed(settled: true) }
  end

  def scenarios # rubocop:disable Metrics/MethodLength,Metrics/AbcSize
    list = []
    add = lambda do |name, job, http, env: ON, raises: false, asset: nil, &setup|
      list << { 'name' => name, 'job' => job, 'http' => http, 'env' => env, 'raises' => raises, 'asset' => asset, 'setup' => setup }
    end
    coingecko_scenarios(add)
    coingecko_ticker_scenarios(add)
    coingecko_category_scenarios(add)

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
    [false, true].each do |stale|
      btc = listing(1, 'base_asset_id' => 'crypto:bitcoin', 'symbol' => 'BTC/USD')
      add.("crypto-collision-#{stale}", job, { key => [ok('data' => healthy + [btc])] }) do
        crypto_seed
        coin = asset('bitcoin', 'BTC', 'Cryptocurrency')
        trust = asset('BTC.US', 'BTC', 'Stock', instrument_type: 'stock')
        ticker(@alpaca, trust, @usd, 'BTC', 'BTC', 'USD')
        ticker(@alpaca, coin, @usd, stale ? '__stale_99_BTC/USD' : 'BTC/USD', stale ? '__stale_99_BTC' : 'BTC', 'USD').update_columns(available: !stale)
      end
    end
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
    [false, true].each do |stale|
      btc = stocks.first.merge('external_id' => 'BTC.US', 'symbol' => 'BTC', 'type' => 'stock',
                               'identifiers' => [{ 'scheme' => 'alpaca', 'value' => 'us_equity:BTC' }])
      btc_listing = listings.first.merge('ticker' => 'BTC', 'base' => 'BTC', 'base_external_id' => 'BTC.US')
      add.("stocks-collision-#{stale}", job,
           { assets_key => [ok('data' => stocks + [btc])], listings_key => [ok('data' => listings + [btc_listing])] }) do
        stock_seed
        coin = asset('bitcoin', 'BTC', 'Cryptocurrency')
        trust = asset('BTC.US', 'BTC', 'Stock', instrument_type: 'stock')
        ticker(@alpaca, coin, @usd, 'BTC/USD', 'BTC', 'USD')
        ticker(@alpaca, trust, @usd, stale ? '__stale_99_BTC' : 'BTC', stale ? '__stale_99_BTC' : 'BTC', 'USD').update_columns(available: !stale)
      end
    end
    add.('stocks-off-switch', job, {}) do
      stock_seed
      AppConfig.set('stock_sync_enabled', 'false')
    end
    add.('stocks-legacy-row', job, both) { stock_seed(legacy: true) }
    # backfill_canonical_stock_external_ids! over a mixed install: rewritten, defensive skips, unmatched, ambiguous, LIKE's
    # case-insensitive wildcard, a non-stock row; then the listings' legacy collision guard and the sweep's legacy exclusion.
    renamed = [
      stocks.first.merge('external_id' => 'IBIT.US', 'symbol' => 'IBIT', 'identifiers' => [{ 'scheme' => 'alpaca', 'value' => 'us_equity:IBIT' }]),
      stocks.first.merge('external_id' => 'LDRC.US', 'symbol' => 'LDRC',
                         'identifiers' => [{ 'scheme' => 'alpaca', 'value' => 'us_equity:LDRC' }, { 'scheme' => 'alpaca', 'value' => 'us_equity:IBIT' },
                                           { 'scheme' => 'figi', 'value' => 'BBG000000001' }])
    ]
    ibit_listing = listings.first.merge('ticker' => 'IBIT', 'base' => 'IBIT', 'base_external_id' => 'LDRC.US')
    add.('stocks-legacy-mix', job, { assets_key => [ok('data' => stocks + renamed)], listings_key => [ok('data' => listings + [ibit_listing])] }) do
      stock_seed
      ticker(@alpaca, asset('alpaca_a1', 'S0002', 'Stock'), @usd, 'S0002', 'S0002', 'USD') # rewritten: its ticker refreshes
      asset('S0003.US', 'S0003', 'Stock')
      ticker(@alpaca, asset('alpaca_a2', 'S0003', 'Stock'), @usd, 'S0003', 'S0003', 'USD') # canonical present: kept, listing dropped
      ticker(@alpaca, asset('alpaca_a3', 'GONE', 'Stock'), @usd, 'GONE', 'GONE', 'USD')    # unmatched: never swept
      ticker(@alpaca, asset('alpaca_a4', 'IBIT', 'Stock'), @usd, 'IBIT', 'IBIT', 'USD')    # ambiguous: kept, listing dropped
      asset('alpaca_a5', 'LDRC', 'Stock')                                                  # its canonical carries two ids
      asset('alpaca_a6', 'S0004', 'Stock')                                                 # rewritten
      asset('alpaca_a7', 'S0004', 'Stock')                                                 # the canonical now exists
      asset('ALPACAxb1', 'S0005', 'Stock')                                                 # LIKE: rewritten
      asset('alpaca_c1', 'S0006', 'Cryptocurrency')                                        # not a stock
    end
    bare = stocks.map { |s| s.except('identifiers') }
    legacy = lambda do |flagged|
      stock_seed(legacy: true)
      AppConfig.set('stock_canonical_backfill_completed_at', '2026-08-01T00:00:00Z') if flagged
    end
    [false, true].each do |flagged|
      tag = flagged ? '-flagged' : ''
      add.("stocks-legacy-no-identifiers#{tag}", job, { assets_key => [ok('data' => bare)], listings_key => [ok('data' => listings)] }) { legacy.(flagged) }
      add.("stocks-legacy-backfill-fail#{tag}", job,
           { assets_key => [error(500, { 'error' => 'boom' }), ok('data' => stocks)], listings_key => [ok('data' => listings)] }) { legacy.(flagged) }
    end
    # Running bots on legacy rows the Alpaca asset sync left without an instrument_type: Rails completes the migration,
    # whether or not another running bot sits on a legacy row the backfill cannot resolve.
    add.('stocks-legacy-running-bot', job, both) do
      stock_seed
      a = asset('alpaca_r1', 'S0002', 'Stock')
      ticker(@alpaca, a, @usd, 'S0002', 'S0002', 'USD')
      running_bot(a)
    end
    add.('stocks-legacy-running-bots-unresolved', job, both) do
      stock_seed
      a = asset('alpaca_r1', 'S0002', 'Stock')
      ticker(@alpaca, a, @usd, 'S0002', 'S0002', 'USD')
      running_bot(a)
      gone = asset('alpaca_r2', 'GONE', 'Stock')
      ticker(@alpaca, gone, @usd, 'GONE', 'GONE', 'USD')
      running_bot(gone)
    end
    # A non-string canonical id takes the string column's cast ("f"), in the backfill as in the upsert: one identity.
    falsy = { 'external_id' => false, 'type' => 'stock', 'symbol' => 'FALSY', 'name' => 'Falsy',
              'identifiers' => [{ 'scheme' => 'alpaca', 'value' => 'us_equity:FALSY' }] }
    add.('stocks-legacy-false-id', job, { assets_key => [ok('data' => stocks + [falsy])], listings_key => [ok('data' => listings)] }) do
      stock_seed
      asset('alpaca_f1', 'FALSY', 'Stock')
    end
    # Object-valued identifiers raise while the map is built: nothing is rewritten and no flag is written.
    malformed = { 'external_id' => 'MSFT.US', 'type' => 'stock', 'symbol' => 'MSFT', 'name' => 'Microsoft',
                  'identifiers' => { 'scheme' => 'alpaca', 'value' => 'us_equity:MSFT' } }
    add.('stocks-legacy-malformed-identifiers', job, { assets_key => [ok('data' => stocks + [malformed])], listings_key => [ok('data' => listings)] },
         raises: true) do
      stock_seed(legacy: true)
    end
    add.('stocks-legacy-backfill-rate-limited', job, { assets_key => [error(429, { 'error' => 'slow' })] }) { legacy.(true) }
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
        'market_cap' => 2.5e13, 'weight' => 0, 'weights' => { 'AAPL.US' => 0.0812, 'MSFT.US' => 0.0799, 'T&<>.US' => 0.01 } },
      { 'external_id' => 'layer-1', 'source' => 'coingecko', 'name' => 'Layer 1', 'description' => 'L1s', 'top_coins' => nil, 'market_cap' => '123.5' },
      { 'external_id' => 'meme-token', 'source' => 'coingecko', 'name' => 'Meme', 'weight' => nil, 'market_cap' => 42 },
      { 'external_id' => 'other', 'source' => 'coingecko', 'name' => 'Other', 'market_cap' => nil }
    ]
    # String#to_d reads a string's leading numeric prefix and nothing else.
    { 'prefix' => '1.5abc', 'garbage' => 'abc', 'blank' => '', 'exponent' => '1e5x', 'spaced' => ' 2', 'underscored' => '1_000.2_5x',
      'dot' => '1.e2', 'signs' => '--1', 'bare-dot' => '.e2', 'lead-dot' => '-.5e1_0' }.each do |id, cap|
      indices << { 'external_id' => "cap-#{id}", 'source' => 'coingecko', 'name' => id, 'market_cap' => cap }
    end
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
    SchemaTemplate.install(dir)
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
    travel_to(Time.iso8601('2026-09-01T00:00:00Z')) { scenario['setup']&.call }
  end

  # The scripted adapters (data-api, CoinGecko, the venues' public catalogues); Coingecko#get_exchange_tickers_by_id's
  # `sleep 3` between pages and the client's rate limit skipped (timing only: the Rust crawl is paced by its throttle).
  def install_harness
    return if @harness

    @harness = true
    Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(ScriptedDataApi::Adapter)
    Faraday::Adapter.lookup_middleware(:net_http).prepend(ScriptedDataApi::Adapter) # Clients::Coingecko's plan ping
    Coingecko.prepend(Module.new { private def sleep(*) = 0 })
    Index::SyncFromCoingeckoJob.prepend(Module.new { private def sleep(*) = 0 }) # its `sleep(3)` between categories
    # The per-plan rate limit waits on Time.now, which travel_to freezes: a 31st request of a run would wait forever.
    FaradayRateLimitMiddleware.prepend(Module.new { def call(env) = @app.call(env) })
  end

  def grid(root)
    install_harness
    list = scenarios
    list.each do |sc|
      dir = File.join(root, sc['name'])
      FileUtils.mkdir_p(dir)
      build(dir, sc)
      enc = ActiveRecord::Encryption.config
      File.write(File.join(dir, 'scenario.json'), JSON.pretty_generate({
        'parity_scratch' => true, 'job' => sc['job'], 'at' => AT, 'env' => sc['env'], 'script' => sc['http'], 'raises' => sc['raises'], 'asset' => sc['asset'],
        # The keys this Rails encrypted the scenario's app_configs with, so the Rust side reads them as Rails does.
        'encryption' => { 'primary_key' => Array(enc.primary_key).first, 'key_derivation_salt' => enc.key_derivation_salt }
      }))
      ActiveRecord::Base.connection_pool.disconnect!
    end
    puts "built #{list.size} scenarios in #{root}"
  end

  # SyncAllTickersAndAssetsJob only enqueues one job per venue, a minute apart: each runs now, in id order, and fails alone.
  def perform(job, asset = nil)
    if job == 'fetch_data_from_coingecko_job'
      Asset::FetchDataFromCoingeckoJob.perform_now(Asset.find_by!(external_id: asset))
    elsif job == 'sync_all_tickers_and_assets_job'
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
    install_harness
    Dir[File.join(root, '*/scenario.json')].sort.each do |path|
      dir = File.dirname(path)
      sc = JSON.parse(File.read(path))
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
      ActiveJob::Base.queue_adapter.enqueued_jobs.clear
      %w[MARKET_DATA_URL MARKET_DATA_TOKEN].each { |k| sc['env'].key?(k) ? ENV[k] = sc['env'][k] : ENV.delete(k) }
      ScriptedDataApi.http = sc['script'].transform_values(&:dup)
      ScriptedDataApi.requests = []
      # Clients::Coingecko and MarketData memoise the plan and the client per key for the process: each scenario pings anew.
      Clients::Coingecko.instance_variable_set(:@detect_plan, nil)
      MarketData.instance_variable_set(:@coingecko, nil)
      # Each scenario's own cache: no venue catalogue, crawl claim or backfill claim carried over (null: every claim taken).
      Rails.cache = ActiveSupport::Cache::NullStore.new
      before = snapshot
      raised = begin
        travel_to(Time.iso8601(sc['at']), with_usec: true) { perform(sc['job'], sc['asset']) }
        false
      rescue StandardError => e
        raise unless sc['raises']

        e
      end
      raise "#{dir}: the job was expected to raise" if sc['raises'] && !raised
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
