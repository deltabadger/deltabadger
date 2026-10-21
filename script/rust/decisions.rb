# The Rails half of the decision-parity harness (rust/tests/parity.rs, script/rust/parity_on_copy.sh).
#   bin/rails runner script/rust/decisions.rb grid <root>    # one install per scenario, built with Rails' own models
#   bin/rails runner script/rust/decisions.rb grid-alpaca <root> # the same for Alpaca, scripted beneath Clients::Alpaca at the Faraday adapter
#   bin/rails runner script/rust/decisions.rb grid-basket <root>  # Alpaca crypto baskets, with the engine's recovery and the next checkpoint
#   bin/rails runner script/rust/decisions.rb grid-limit <root>   # the amount limit, its stop and its mail, on Alpaca crypto, Kraken, Binance and KuCoin
#   bin/rails runner script/rust/decisions.rb grid-binance <root> # Binance spot, scripted beneath honeymaker at the Faraday adapter
#   bin/rails runner script/rust/decisions.rb grid-binance-us <root> # Binance.US spot: the Binance grid at api.binance.us
#   bin/rails runner script/rust/decisions.rb grid-kucoin <root>  # KuCoin spot, scripted beneath honeymaker at the Faraday adapter
#   bin/rails runner script/rust/decisions.rb grid-bitvavo <root> # Bitvavo spot, scripted beneath honeymaker at the Faraday adapter
#   bin/rails runner script/rust/decisions.rb grid-gemini <root>  # Gemini spot, scripted beneath honeymaker at the Faraday adapter (plan VB B3)
#   bin/rails runner script/rust/decisions.rb grid-sell <root>    # a one-asset basket selling on Kraken, Binance and Alpaca (slice 6a)
#   bin/rails runner script/rust/decisions.rb grid-venue-basket <root> # buying baskets of 2 and 3 members on Kraken (EUR) and Binance (USDT)
#   bin/rails runner script/rust/decisions.rb grid-venue-index <root>  # crypto index bots on Kraken, Binance, KuCoin and Alpaca, and their resync (plan CI)
#   bin/rails runner script/rust/decisions.rb grid-basket-sell <root> # a wider Alpaca basket's ranked DCA-out (slice 6b)
#   bin/rails runner script/rust/decisions.rb grid-rebalance <root>   # threshold rebalancing on a stopped Alpaca basket (slice 6b)
#   bin/rails runner script/rust/decisions.rb grid-conditions <root> # price_limit / sell_price_limit with `pause`, and the minute check (slice 7a)
#   bin/rails runner script/rust/decisions.rb grid-signal <root>  # Bot::SignalJob, one webhook call of a signal bot, on all four venues (slice 8a)
#   bin/rails runner script/rust/decisions.rb record <root>  # Rails' ticks (and retries) per <root>/<scenario>/ -> rails.json
# Always run with every *_DATABASE_URL pointing at scratch files and PROXY_KRAKEN at a dead address. Kraken is
# scripted beneath the real client (Honeymaker::Clients::Kraken#get_public/#post_private), so every line of
# Rails' own parsing runs. Any path a scenario did not script raises Harness::Unscripted, on either venue, and so does any
# other real connection (a Net::HTTP#connect backstop beneath every client).
require 'json'
require 'fileutils'
require 'active_support/testing/time_helpers'
require_relative 'schema_template'

module Harness
  # An Exception, not a StandardError: with_rescue / honeymaker would turn a harness gap into an ordinary Failure.
  class Unscripted < Exception; end # rubocop:disable Lint/InheritException
end

# A run the Rust side's watchdog finds stuck (tests/common/decisions.rs) is asked for every thread's backtrace first.
Signal.trap('USR1') do
  Thread.list.each { |thread| $stderr.puts("#{thread.inspect}\n  #{(thread.backtrace || []).first(60).join("\n  ")}") }
  $stderr.flush
end
# The backstop for every command (grid builds run model callbacks too): no real Net::HTTP connection, ever.
# ponytail: Net::HTTP only; httpx (eth, Hyperliquid signing) and raw sockets bypass it, and no Kraken/Alpaca tick reaches them.
Net::HTTP.prepend(Module.new { def connect = raise(Harness::Unscripted, "real connection to #{address}:#{port}") })

module ScriptedKraken
  mattr_accessor :http, :sent

  def self.reply(path, params)
    sent << params.transform_keys(&:to_s).except('nonce').compact if path == '/0/private/AddOrder'
    # A public call scripted with its query ("/0/public/Ticker?pair=ETHEUR") is answered first: a basket prices each member.
    keyed = "#{path}?#{params.compact.map { |k, v| "#{k}=#{v}" }.sort.join('&')}" if path.start_with?('/0/public/')
    queue = (keyed && http&.[](keyed)) || http&.[](path) or raise Harness::Unscripted, "unscripted Kraken call #{path}"
    Result::Success.new(queue.size > 1 ? queue.shift : queue.first)
  end

  module Http
    # No `http` (a `kraken_http` scenario): the real client, down to the Faraday adapter (ScriptedKrakenHttp).
    def get_public(path, params = {}) = ScriptedKraken.http ? ScriptedKraken.reply(path, params) : super
    def post_private(path, body = {}) = ScriptedKraken.http ? ScriptedKraken.reply(path, body) : super
  end
end

# Kraken at the Faraday adapter beneath Honeymaker::Clients::Kraken, for the scenarios that script `kraken_http` (the crypto
# index grid): honeymaker's own with_rescue (an HTTP status, a transport failure) and Exchanges::Kraken's parsing all run.
# A request is named "METHOD path" (a public one with its query); the order Rails POSTs is recorded as its form, without the
# nonce that changes per send.
module ScriptedKrakenHttp
  mattr_accessor :http, :sent

  def self.reply(env)
    key = "#{env.method.to_s.upcase} #{env.url.path}"
    sent << URI.decode_www_form(env.request_body.to_s).to_h.except('nonce') if key == 'POST /0/private/AddOrder'
    keyed = "#{key}?#{Rack::Utils.parse_query(env.url.query).map { |k, v| "#{k}=#{v}" }.sort.join('&')}" if env.url.query
    queue = (keyed && http[keyed]) || http[key] or raise Harness::Unscripted, "unscripted Kraken call #{keyed || key}"
    queue.size > 1 ? queue.shift : queue.first
  end
end

# Alpaca is scripted one layer lower than Kraken: beneath Clients::Alpaca, at the Faraday adapter its connections use, so
# Rails' own middleware (json, raise_error), Clients::Alpaca#with_rescue, Client.network_failure and Exchanges::Alpaca's
# parsing all run. Only while an Alpaca scenario runs (http set); a Kraken scenario never reaches this adapter.
module ScriptedAlpaca
  mattr_accessor :http, :sent

  NETWORK = {
    'pre_send_data' => -> { Faraday::ConnectionFailed.new(Errno::ECONNREFUSED.new('connect(2) for "data.alpaca.markets" port 443')) },
    'pre_send' => -> { Faraday::ConnectionFailed.new(Errno::ECONNREFUSED.new('connect(2) for "paper-api.alpaca.markets" port 443')) },
    'binance_pre_send' => -> { Faraday::ConnectionFailed.new(Errno::ECONNREFUSED.new('connect(2) for "api.binance.com" port 443')) },
    'kucoin_pre_send' => -> { Faraday::ConnectionFailed.new(Errno::ECONNREFUSED.new('connect(2) for "api.kucoin.com" port 443')) },
    'kraken_pre_send' => -> { Faraday::ConnectionFailed.new(Errno::ECONNREFUSED.new('connect(2) for "api.kraken.com" port 443')) },
    'post_send' => -> { Faraday::TimeoutError.new(Net::ReadTimeout.new) },
    # Client.network_failure returns a Failure for an SSL cause (not retried): "Faraday::SSLError: certificate verify failed".
    'permanent' => -> { Faraday::SSLError.new(OpenSSL::SSL::SSLError.new('certificate verify failed')) }
  }.freeze

  # A scripted transport failure: NETWORK's kinds, or `pre_send:<host>`, a connection to that venue's host refused (every
  # venue a slice ports spells its own so; the older `<venue>_pre_send` kinds stay for the grids recorded with them).
  def self.network(kind)
    host = kind.delete_prefix('pre_send:') if kind.start_with?('pre_send:')
    return Faraday::ConnectionFailed.new(Errno::ECONNREFUSED.new("connect(2) for \"#{host}\" port 443")) if host

    NETWORK.fetch(kind).call
  end

  def self.reply(env)
    raise Harness::Unscripted, "unscripted HTTP call to #{env.url}" unless env.url.host.to_s.end_with?('alpaca.markets')

    key = "#{env.method.to_s.upcase} #{env.url.path}"
    sent << JSON.parse(env.request_body).except('client_order_id') if key == 'POST /v2/orders'
    queue = http[key] or raise Harness::Unscripted, "unscripted Alpaca call #{key}"
    queue.size > 1 ? queue.shift : queue.first
  end

  module Adapter
    def call(env)
      # Every honeymaker client uses the same adapter: a venue's host is answered by its ScriptedVenue (VENUES).
      scripted = VENUES.values.to_h { |v| [v.scripted.host, v.scripted] }.merge('api.kraken.com' => ScriptedKrakenHttp).fetch(env.url.host, ScriptedAlpaca)
      raise Harness::Unscripted, "unscripted HTTP call #{env.method.to_s.upcase} #{env.url}" if scripted.http.nil?

      reply = scripted.reply(env)
      if (kind = reply['network'])
        error = ScriptedAlpaca.network(kind)
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

# A venue scripted like Alpaca, at the Faraday adapter beneath its honeymaker client, so honeymaker's own signing, middleware
# (json, raise_error), #with_rescue and the Exchange's parsing all run. A request is named "METHOD path"; one scripted with
# its query ("GET /api/v3/ticker/bookTicker?symbol=ETHUSDT") is answered first: a basket prices each member (signed calls
# carry a timestamp, so they only ever match their bare key). The order Rails sends to `order` is recorded as its query
# (`body: :query`) or its JSON body, without the fields that change per send (`strip`: a timestamp and signature, or the
# client order id Rails draws fresh while Rust sends the intent id).
class ScriptedVenue
  attr_accessor :http, :sent
  attr_reader :host, :name, :order

  def initialize(host:, name:, order:, body:, strip:)
    @host, @name, @order, @body, @strip = host, name, order, body, strip
  end

  def reply(env)
    key = "#{env.method.to_s.upcase} #{env.url.path}"
    sent << sent_fields(env).except(*@strip) if key == order
    keyed = "#{key}?#{Rack::Utils.parse_query(env.url.query).map { |k, v| "#{k}=#{v}" }.sort.join('&')}" if env.url.query
    queue = (keyed && http[keyed]) || http[key] or raise Harness::Unscripted, "unscripted #{name} call #{key}"
    queue.size > 1 ? queue.shift : queue.first
  end

  # The order's fields as the venue reads them: its query, its JSON body, or (Gemini) the base64 JSON payload of its
  # empty-body POST.
  def sent_fields(env)
    case @body
    when :query then Rack::Utils.parse_query(env.url.query)
    when :payload then JSON.parse(Base64.strict_decode64(env.request_headers['X-GEMINI-PAYLOAD']))
    else JSON.parse(env.request_body)
    end
  end
end

# The venues scripted beneath honeymaker (plan VB §1.8), one row each: its scripted host, the exchange row and BTC pair a
# scenario builds (as the venue's #get_tickers_info stores it), the quote asset, the key's passphrase, and the reply that
# never reached it. A venue slice adds its row here; its grids' bodies stay the grid's own.
VenueRow = Struct.new(:scripted, :exchange, :name, :ticker, :quote, :passphrase, :pre_send, keyword_init: true) do
  # The scripted reply to a request that never left: Client.network_failure's text for a refused connection.
  def not_sent = { 'network' => pre_send || "pre_send:#{scripted.host}",
                   'message' => "Faraday::ConnectionFailed: Connection refused - connect(2) for \"#{scripted.host}\" port 443" }
end
TETHER = %w[tether USDT Tether Cryptocurrency].freeze
EURO = %w[EUR.FOREX EUR Euro Currency].freeze
VENUES = {
  'binance' => VenueRow.new(scripted: ScriptedVenue.new(host: 'api.binance.com', name: 'Binance', order: 'POST /api/v3/order', body: :query, strip: %w[timestamp signature]),
                            exchange: 'Exchanges::Binance', name: 'Binance', ticker: 'BTCUSDT', quote: TETHER, passphrase: nil, pre_send: 'binance_pre_send'),
  'binance_us' => VenueRow.new(scripted: ScriptedVenue.new(host: 'api.binance.us', name: 'Binance.US', order: 'POST /api/v3/order', body: :query, strip: %w[timestamp signature]),
                               exchange: 'Exchanges::BinanceUs', name: 'Binance.US', ticker: 'BTCUSDT', quote: TETHER, passphrase: nil),
  'kucoin' => VenueRow.new(scripted: ScriptedVenue.new(host: 'api.kucoin.com', name: 'KuCoin', order: 'POST /api/v1/orders', body: :json, strip: %w[clientOid]),
                           exchange: 'Exchanges::Kucoin', name: 'KuCoin', ticker: 'BTC-USDT', quote: TETHER, passphrase: 'p', pre_send: 'kucoin_pre_send'),
  # Plan VB slice A3: Rails sends no clientOrderId (Rust sends the intent id, stripped from both sides).
  'bitvavo' => VenueRow.new(scripted: ScriptedVenue.new(host: 'api.bitvavo.com', name: 'Bitvavo', order: 'POST /v2/order', body: :json, strip: %w[clientOrderId]),
                            exchange: 'Exchanges::Bitvavo', name: 'Bitvavo', ticker: 'BTC-EUR', quote: EURO, passphrase: nil),
  # Gemini's params travel in the signed payload: recorded without `request` (the path), `nonce` (the clock) and
  # `client_order_id` (Rails sends none; Rust sends the intent id).
  'gemini' => VenueRow.new(scripted: ScriptedVenue.new(host: 'api.gemini.com', name: 'Gemini', order: 'POST /v1/order/new', body: :payload,
                                                       strip: %w[request nonce client_order_id]),
                           exchange: 'Exchanges::Gemini', name: 'Gemini', ticker: 'btcusd', quote: %w[usd USD US\ Dollar Currency], passphrase: nil)
}.freeze

module Decisions
  extend ActiveSupport::Testing::TimeHelpers
  module_function

  JSON_COLUMNS = %w[settings transient_data details error_messages].freeze
  MAX_ATTEMPTS = 6
  # What only the Rust engine writes into transient_data: its placement intent, and the mails it owes (rust/src/engine/notice.rs).
  MAIL_MARKERS = %w[rust_funds_mail_pending rust_error_mail_pending rust_stopped_mail_pending rust_limit_mail_pending].freeze
  # Its waits and counted amount-limit stops are the engine's own too (rust/src/engine/placement.rs, polling.rs).
  RUST_KEYS = (%w[rust_placement rust_signal_calls rust_defer_until rust_amount_limit_stops_pending run_generation tick_generation rust_rebalance_halted
                  rust_index_resync] + MAIL_MARKERS).freeze
  # One marker of each kind, as the engine leaves them: Rails must carry them through every write of a tick, untouched.
  SEEDED_MARKERS = { 'rust_funds_mail_pending' => { 'quote_asset' => 2, 'stamped_at' => '2026-08-31T09:00:00.000Z' },
                     'rust_error_mail_pending' => { 'unknown' => { 'error' => 'an <old> "error"', 'stamped_at' => '2026-08-31T09:00:00.000Z' } },
                     'rust_stopped_mail_pending' => { 'error' => 'unauthorized.', 'stamped_at' => '2026-08-31T09:00:00.000Z' },
                     'rust_limit_mail_pending' => { 'stamped_at' => '2026-08-31T09:00:00.000Z' } }.freeze

  def raw(value) = value.is_a?(Float) ? { 'f' => [value].pack('G').unpack1('H*') } : value

  def rows(table)
    ActiveRecord::Base.connection.select_all("SELECT * FROM #{table} ORDER BY id").to_a.to_h do |r|
      r = r.transform_values { |v| raw(v) }
      # Rust-only run ownership attribution, asserted by placement/tick_basket protocol tests.
      r.delete('run_identity') if table == 'transactions'
      JSON_COLUMNS.each { |c| r[c] = JSON.parse(r[c]) if r[c].is_a?(String) }
      r['transient_data'] = r['transient_data'].except(*RUST_KEYS) if r['transient_data'].is_a?(Hash)
      [r['id'], r]
    end
  end

  # The tick rewrites a basket's members (Bot::Composition::Allocatable#update_bot_index_assets), so they are compared too.
  def snapshot = %w[bots transactions bot_activity_logs bot_index_assets].to_h { |t| [t, rows(t)] }

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

  # A VENUES row's exchange, BTC and its quote listed, and the BTC pair as `sc['ticker']` gives its precisions.
  def venue_row(row, sc)
    exchange = row.exchange.constantize.create!(name: row.name, maker_fee: '0.1', taker_fee: '0.1')
    btc = Asset.create!(external_id: 'bitcoin', symbol: 'BTC', name: 'Bitcoin', category: 'Cryptocurrency')
    external_id, symbol, name, category = row.quote
    quote = Asset.create!(external_id:, symbol:, name:, category:)
    [btc, quote].each { |a| ExchangeAsset.create!(exchange:, asset: a, available: true) }
    ticker = Ticker.create!(exchange:, ticker: row.ticker, base: 'BTC', quote: symbol, base_asset: btc, quote_asset: quote, **sc['ticker'].symbolize_keys)
    [exchange, btc, quote, ticker, row.passphrase]
  end

  # The other venue a merge brought rows from (Bot::Merge): a row marked `foreign` was placed there.
  def foreign_venue = Exchanges::Kraken.find_or_create_by!(name: 'Kraken') { |k| k.assign_attributes(maker_fee: '0.25', taker_fee: '0.4') }

  def build(dir, sc)
    SchemaTemplate.install(dir)
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
    user = User.new(name: 'Owner', email: 'owner@example.com', encrypted_password: SchemaTemplate.owner_password('correct horse battery staple'), admin: true,
                    confirmed_at: Time.current, setup_completed: true)
    user.save!(validate: false)
    exchange, btc, quote, ticker, passphrase = case sc['venue']
                                               when 'alpaca' then alpaca_venue(sc)
                                               when *VENUES.keys then venue_row(VENUES.fetch(sc['venue']), sc)
                                               else kraken_venue(sc)
                                               end
    # A basket's other members, created after BTC so that a one-asset scenario's ids are what they were.
    assets = { 'BTC' => btc }
    (sc['members'] || {}).each_key { |sym| assets[sym] ||= basket_member(exchange, quote, sym, sc['venue']) }
    # Plan CB T5: the members' market caps as the asset sync stores them, before the save derives a market-cap basket's weights.
    (sc['market_caps'] || {}).each { |sym, cap| assets.fetch(sym).update_columns(market_cap: cap) }
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
                         'transient_data' => bot.reload.transient_data.merge(sc['transient']),
                         'last_end_of_funds_notification' => sc['funds_notified_at'] && Time.iso8601(sc['funds_notified_at']) }
                       .merge(sc['bot_columns'] || {}))
    sc['transactions'].each do |t|
      asset = assets.fetch(t.fetch('asset', 'BTC'))
      Transaction.insert!(t.except('asset', 'foreign').merge('bot_id' => bot.id, 'exchange_id' => t['foreign'] ? foreign_venue.id : exchange.id,
                                                  'base_asset_id' => asset.id, 'quote_asset_id' => quote.id,
                                                  'base' => asset.symbol, 'quote' => quote.symbol, 'side' => t.fetch('side', 0), 'transaction_type' => t.fetch('transaction_type', 'REGULAR'),
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
        # An order QueryOrders dropped, placed long before, that a capped TradesHistory walk (20 pages, the last one repeating)
        # did not see. Over 1,000 trades in the window it may have filled further back and stays waiting; at 1,000 the walk read
        # them all, so it is never-executed and abandoned (Exchanges::Kraken#recover_missing_from_trades).
        **[['history_beyond_reach', 1_001], ['history_within_reach', 1_000]].to_h do |name, count|
          [name, { 'at' => after.(1),
                   'transactions' => [{ 'status' => 0, 'external_status' => 0, 'external_id' => 'OFAR-1', 'order_type' => 0, 'quote_amount' => '60',
                                        'price' => '50000', 'created_at' => '2026-08-01 10:00:01' }],
                   'http' => { '/0/private/TradesHistory' => [
                     { 'error' => [], 'result' => { 'count' => count, 'trades' => { 'T1' => { 'ordertxid' => 'OTHER-1', 'vol' => '0.0006', 'cost' => '30', 'fee' => '0.078', 'type' => 'buy', 'ordertype' => 'market', 'pair' => 'XXBTZEUR' } } } }] } }]
        end,
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
                             'http' => { '/0/public/Ticker' => [ticker_body('9.9', '10.1', '10.0')] } },
        # A starting time as Bot::Lifecycle#start leaves it (settings.start_at = started_at, the chosen 10:00 in the owner's UTC
        # zone, the carry captured as 0): the first run turns the rule off (Bot::Startable#disable_starting_time!), and the
        # next checkpoint's run follows.
        'start_time_first_tick' => { 'started_at' => '2026-09-01T10:00:00Z', 'at' => '2026-09-01T10:00:00.500000Z',
                                     'next_at' => (Time.iso8601('2026-09-01T10:00:00Z') + step + 1.second).iso8601(6),
                                     'settings' => { 'start_time_enabled' => true, 'start_time_mode' => 'hour', 'start_time_of_day' => '10:00',
                                                     'start_at' => '2026-09-01T10:00:00Z' },
                                     'settings_changed_at' => '2026-08-31T09:00:00Z', 'transient' => { 'missed_quote_amount' => 0 } }
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
            'started_at' => v.fetch('started_at', started), 'settings' => settings.merge(v.fetch('settings', {})),
            'settings_changed_at' => v['settings_changed_at'],
            'transient' => v.fetch('transient', {}), 'transactions' => v.fetch('transactions', []), 'ticker' => v.fetch('ticker', ticker),
            'ticker_after' => v['ticker_after'], 'at' => v.fetch('at'), 'script' => { 'http' => http.merge(v_http) },
            'tick' => v.fetch('tick', true), 'poll' => v['poll'], 'next_at' => v['next_at'] }
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

  # A basket's member beside BTC, spelled as its venue lists it: SYM/USD on Alpaca, SYMEUR on Kraken, SYMUSDT on Binance.
  def basket_member(exchange, quote, sym, venue)
    asset = Asset.create!(external_id: sym.downcase, symbol: sym, name: sym, category: 'Cryptocurrency')
    ExchangeAsset.create!(exchange:, asset:, available: true)
    ticker, pair = case venue
                   when 'kraken', 'binance' then ["#{sym}#{quote.symbol}", VENUE_PAIRS.fetch(venue).fetch(sym)]
                   when 'kucoin' then ["#{sym}-#{quote.symbol}", VENUE_PAIRS.fetch(venue).fetch(sym)]
                   else ["#{sym}/USD", BASKET_PAIRS.fetch(sym)]
                   end
    Ticker.create!(exchange:, ticker:, base: sym, quote: quote.symbol, base_asset: asset, quote_asset: quote, **pair.symbolize_keys)
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
    # The same buy part-filled: Alpaca's partially_filled is open, so the sweep records the fill and the tick goes on.
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
             next_at: basket_at(1),
             http: swept.merge('POST /v2/orders' => posts, 'GET /v2/orders:by_client_order_id' => [lookup]))
    end
    settings = limit_for ? { 'quote_amount_limited' => true, 'quote_amount_limit' => limit_for.(WEIGHTS['tiers']) } : {}
    transient = stamp ? { 'quote_amount_limit_enabled_at' => stamp } : {}
    list << landed_reference("#{prefix}-recover-tiers-k2-landed", WEIGHTS['tiers'], legs: [%w[OTX-1 BTC 60]], landed: %w[ETH 36],
                             quote_amount: 120.0, settings:, transient:)
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
    # Market-cap weights (plan CB T5, 5).
    market_cap_cases(basket_first, basket_at(1), [closed_leg(1, 'BTC', 60.0)]).each do |name, members, opts|
      list << basket("basket-market_cap-#{name}", members, **opts)
    end
    list
  end

  # P1-T0: buying baskets on Kraken (EUR) and Binance (USDT). Each member has its venue's own spelling (Kraken's XBT), its
  # own minimums and decimals, and is priced by its own scripted reply.
  VENUE_PAIRS = {
    'kraken' => {
      'BTC' => { 'base_decimals' => 8, 'quote_decimals' => 5, 'price_decimals' => 1, 'minimum_base_size' => '0.00005', 'minimum_quote_size' => '0.5' },
      'ETH' => { 'base_decimals' => 8, 'quote_decimals' => 5, 'price_decimals' => 2, 'minimum_base_size' => '0.002', 'minimum_quote_size' => '0.5' },
      'SOL' => { 'base_decimals' => 7, 'quote_decimals' => 4, 'price_decimals' => 3, 'minimum_base_size' => '0.05', 'minimum_quote_size' => '0.75' }
    },
    'binance' => {
      'BTC' => { 'base_decimals' => 5, 'quote_decimals' => 8, 'price_decimals' => 2, 'minimum_base_size' => '0.00001', 'minimum_quote_size' => '5' },
      'ETH' => { 'base_decimals' => 4, 'quote_decimals' => 8, 'price_decimals' => 2, 'minimum_base_size' => '0.0001', 'minimum_quote_size' => '5' },
      'SOL' => { 'base_decimals' => 3, 'quote_decimals' => 8, 'price_decimals' => 3, 'minimum_base_size' => '0.001', 'minimum_quote_size' => '6' }
    },
    # Plan CB: KuCoin's USDT pairs (baseMinSize, quoteMinSize, the increments' decimals).
    'kucoin' => {
      'BTC' => { 'base_decimals' => 8, 'quote_decimals' => 6, 'price_decimals' => 1, 'minimum_base_size' => '0.00001', 'minimum_quote_size' => '0.1' },
      'ETH' => { 'base_decimals' => 7, 'quote_decimals' => 6, 'price_decimals' => 2, 'minimum_base_size' => '0.0001', 'minimum_quote_size' => '0.1' },
      'SOL' => { 'base_decimals' => 4, 'quote_decimals' => 6, 'price_decimals' => 3, 'minimum_base_size' => '0.01', 'minimum_quote_size' => '0.1' }
    }
  }.freeze
  # [bid, ask, last] per member and venue.
  VENUE_PRICES = {
    'kraken' => { 'BTC' => %w[49990.1 50000.2 49995.3], 'ETH' => %w[2499.75 2500.25 2500.01], 'SOL' => %w[149.875 150.125 150.001] },
    'binance' => { 'BTC' => %w[64000.10000000 64000.11000000 64000.05000000], 'ETH' => %w[2500.50000000 2500.51000000 2500.47000000],
                   'SOL' => %w[150.12400000 150.12500000 150.12300000] },
    'kucoin' => { 'BTC' => %w[64000.1 64000.2 64000.15], 'ETH' => %w[2500.5 2500.51 2500.47], 'SOL' => %w[150.124 150.125 150.123] }
  }.freeze
  VENUE_TICKER = { 'kraken' => { 'BTC' => 'XBTEUR', 'ETH' => 'ETHEUR', 'SOL' => 'SOLEUR' },
                   'binance' => { 'BTC' => 'BTCUSDT', 'ETH' => 'ETHUSDT', 'SOL' => 'SOLUSDT' },
                   'kucoin' => { 'BTC' => 'BTC-USDT', 'ETH' => 'ETH-USDT', 'SOL' => 'SOL-USDT' } }.freeze

  def kr_ticker(pair, bid, ask, last) = { 'error' => [], 'result' => { pair => {
    'a' => [ask, '1', '1.000'], 'b' => [bid, '1', '1.000'], 'c' => [last, '0.001'], 'v' => ['12.5', '30.1'], 'p' => [last, last],
    't' => [100, 250], 'l' => [bid, bid], 'h' => [ask, ask], 'o' => last } } }

  # The members' price replies, each keyed by its own pair; `fail` answers one member with the venue's error instead.
  def venue_prices(venue, fail: nil)
    VENUE_PRICES.fetch(venue).flat_map do |sym, (bid, ask, last)|
      pair = VENUE_TICKER.fetch(venue).fetch(sym)
      if venue == 'kraken'
        body = sym == fail ? { 'error' => ['EQuery:Unknown asset pair'] } : kr_ticker(pair, bid, ask, last)
        [["/0/public/Ticker?pair=#{pair}", [body]]]
      else
        bad = { 'status' => 400, 'body' => { 'code' => -1121, 'msg' => 'Invalid symbol.' } }
        [["GET /api/v3/ticker/bookTicker?symbol=#{pair}", [sym == fail ? bad : ok('symbol' => pair, 'bidPrice' => bid, 'bidQty' => '1.5', 'askPrice' => ask, 'askQty' => '0.2')]],
         ["GET /api/v3/ticker/price?symbol=#{pair}", [sym == fail ? bad : ok('symbol' => pair, 'price' => last)]]]
      end
    end.to_h
  end

  def kr_order(pair, status:, vol:, vol_exec:, cost:, price:)
    { 'status' => status, 'vol' => vol, 'vol_exec' => vol_exec, 'cost' => cost, 'price' => price, 'oflags' => 'viqc',
      'descr' => { 'pair' => pair, 'type' => 'buy', 'ordertype' => 'market', 'price' => '0' } }
  end

  def venue_http(venue, fail: nil)
    if venue == 'kraken'
      { '/0/private/AddOrder' => (1..6).map { |n| { 'error' => [], 'result' => { 'txid' => ["OTX-#{n}"] } } },
        '/0/private/BalanceEx' => [balance_body('100000')], '/0/private/QueryOrders' => [query_body({})],
        '/0/private/TradesHistory' => [{ 'error' => [], 'result' => { 'trades' => {}, 'count' => 0 } }] }.merge(venue_prices(venue, fail:))
    else
      { 'POST /api/v3/order' => (1..6).map { |n| ok('symbol' => 'BTCUSDT', 'orderId' => 28_456 + n, 'orderListId' => -1, 'clientOrderId' => 'x', 'transactTime' => 1_788_000_000_000) },
        'GET /api/v3/account' => [ok('makerCommission' => 10, 'canTrade' => true, 'balances' => [{ 'asset' => 'USDT', 'free' => '100000.00000000', 'locked' => '0' }])],
        'GET /api/v3/allOrders' => [ok([])], 'GET /api/v3/myTrades' => [ok([])] }.merge(venue_prices(venue, fail:))
    end
  end

  # A member's closed buy of `value` quote at its ask, as the first contribution left it.
  def venue_leg(venue, n, sym, value)
    ask = VENUE_PRICES.fetch(venue).fetch(sym)[1].to_d
    qty = (value.to_d / ask).round(VENUE_PAIRS.fetch(venue).fetch(sym)['base_decimals'])
    { 'asset' => sym, 'status' => 0, 'external_status' => 2, 'external_id' => "OCLOSED-#{n}", 'order_type' => 0, 'quote_amount' => value.to_d.to_s('F'),
      'quote_amount_exec' => (qty * ask).to_s('F'), 'amount_exec' => qty.to_s('F'), 'price' => ask.to_s('F'), 'created_at' => '2026-09-01 10:00:01' }
  end

  def venue_basket(venue, name, members, at:, quote_amount: 60.0, settings: {}, transient: {}, transactions: [], http: {}, fail: nil, **extra)
    { 'name' => "vb-#{venue}-#{name}", 'venue' => venue, 'interval' => 'day', 'quote_amount' => quote_amount, 'started_at' => BASKET_STARTED,
      'settings' => settings, 'settings_changed_at' => nil, 'transient' => transient, 'transactions' => transactions,
      'ticker' => VENUE_PAIRS.fetch(venue)['BTC'], 'members' => members, 'at' => at,
      'script' => { (venue == 'kraken' ? 'http' : venue) => (venue == 'kucoin' ? cb_http(venue, {}) : venue_http(venue, fail:)).merge(http) }, 'tick' => true,
      'record_floor' => true }.merge(extra.transform_keys(&:to_s))
  end

  # Plan CB T5: the members' caps (bigints, as the asset sync stores them), and the cases of a market-cap basket's derivation.
  MARKET_CAPS = { 'BTC' => 1_300_000_000_000, 'ETH' => 400_000_000_000, 'SOL' => 80_000_000_000 }.freeze

  def market_cap_cases(at_first, at_next, held)
    mc = { 'weighting' => 'market_cap' }
    # SOL parked at 0 %: with every cap it is bought at its cap weight; with one cap missing or zero the sliders stay, so it is not.
    parked = { 'BTC' => 0.6, 'ETH' => 0.4, 'SOL' => 0.0 }
    { 'first_tick' => [WEIGHTS['tiers'], MARKET_CAPS, at_first, []],
      'missing_cap' => [parked, MARKET_CAPS.except('SOL'), at_first, []],
      'zero_cap' => [parked, MARKET_CAPS.merge('ETH' => 0), at_first, []],
      'parked' => [parked, MARKET_CAPS, at_first, []],
      'drifted' => [WEIGHTS['tiers'], MARKET_CAPS, at_next, held] }.map do |name, (members, caps, at, rows)|
      # 300 a day, so SOL's cap share (4.5 %) clears each venue's minimum.
      [name, members, { at:, quote_amount: 300.0, settings: mc, market_caps: caps, transactions: rows }]
    end
  end

  def venue_basket_scenarios
    modes = { 'market' => {}, 'limit' => { 'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.0025 } }
    off = { 'trading_enabled' => false }
    kucoin = market_cap_cases(basket_first, basket_at(1), [venue_leg('kucoin', 1, 'BTC', 60.0)]).map do |name, members, opts|
      venue_basket('kucoin', "market_cap-#{name}", members, **opts)
    end
    crypto = %w[kraken binance].flat_map do |venue|
      small, tiny = venue == 'kraken' ? [20.0, 2.0] : [20.0, 6.0] # SOL's 4 under its minimum; every leg of the 70/30 under its own
      list = []
      %w[w70 tiers].product(modes.keys).each do |w, mode|
        list << venue_basket(venue, "#{w}-first_tick-#{mode}", WEIGHTS[w], at: basket_first, settings: modes[mode])
      end
      list << venue_basket(venue, 'tiers-drifted', WEIGHTS['tiers'], at: basket_at(1), transactions: [venue_leg(venue, 1, 'BTC', 60.0)])
      modes.each do |mode, settings|
        list << venue_basket(venue, "tiers-one_below-#{mode}", WEIGHTS['tiers'], at: basket_first, quote_amount: small, settings:)
      end
      # SOL's leg was below its minimum yesterday: BTC and ETH hold theirs, so today SOL takes its share plus what it missed.
      list << venue_basket(venue, 'tiers-below_carried', WEIGHTS['tiers'], at: basket_at(1), quote_amount: small,
                           transactions: [venue_leg(venue, 1, 'BTC', 10.0), venue_leg(venue, 2, 'ETH', 6.0)])
      list << venue_basket(venue, 'w70-all_below', WEIGHTS['w70'], at: basket_first, quote_amount: tiny)
      list << venue_basket(venue, 'tiers-price_missing-middle', WEIGHTS['tiers'], at: basket_at(1), fail: 'ETH')
      list << venue_basket(venue, 'w70-untradable-one', WEIGHTS['w70'], at: basket_at(1), tickers_after: { 'ETH' => off })
      list << venue_basket(venue, 'tiers-unavailable-one', WEIGHTS['tiers'], at: basket_at(1), tickers_after: { 'SOL' => { 'available' => false } })
      list << venue_basket(venue, 'tiers-smart-first_tick', WEIGHTS['tiers'], at: basket_first,
                           settings: { 'smart_intervaled' => true, 'smart_interval_quote_amount' => 20.0 })
      list << venue_basket(venue, 'tiers-carry', WEIGHTS['tiers'], at: basket_at(1), transient: { 'missed_quote_amount' => '12.5' })
      market_cap_cases(basket_first, basket_at(1), [venue_leg(venue, 1, 'BTC', 60.0)]).each do |name, members, opts|
        list << venue_basket(venue, "market_cap-#{name}", members, **opts)
      end
      next list unless venue == 'kraken'

      # The same day's legs swept closed before tomorrow's split, SOL carried across the two ticks (Kraken: one QueryOrders
      # body answers each txid it is asked for).
      t = VENUE_TICKER['kraken']
      swept = query_body('OTX-1' => kr_order(t['BTC'], status: 'closed', vol: '10', vol_exec: '0.00019999', cost: '10', price: '50000.2'),
                         'OTX-2' => kr_order(t['ETH'], status: 'closed', vol: '6', vol_exec: '0.0023997', cost: '6', price: '2500.25'))
      list << venue_basket(venue, 'tiers-one_below-next', WEIGHTS['tiers'], at: basket_first, quote_amount: small, next_at: basket_at(1),
                           http: { '/0/private/QueryOrders' => [swept] })
    end
    (kucoin + crypto).tap { |list| raise "the venue basket grid has #{list.size} scenarios, not 44" unless list.size == 44 }
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
           transient: { 'quote_amount_limit_enabled_at' => LIMIT_STAMP }.merge(transient), **rest)
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
                             transient: { 'quote_amount_limit_enabled_at' => LIMIT_STAMP })
    list << limited('ambiguous_not_placed', at: basket_first, recover_at: recover, next_at: basket_at(1),
                    http: { 'POST /v2/orders' => [POST_SEND, placed(2, 'BTC')], 'GET /v2/orders:by_client_order_id' => [NOT_FOUND] })
    # The recovery scenarios under a cap.
    list.concat(recover_scenarios(prefix: 'limit', limit_for: ->(members) { members ? 200.0 : 100.0 }, stamp: LIMIT_STAMP))
    list.concat(venue_limit_scenarios)
  end

  # Plan CB T2: the same cap on Kraken (EUR), Binance and KuCoin (USDT), one BTC asset, each venue scripted by its own bodies:
  # [script key, pair, the tick's placed id, a waiting row's external id, sweep/poll replies for a resting limit, an open
  # market order, and a fill of `quote` for `qty` at `price` with a quote fee (Kraken `cost` and KuCoin `dealFunds` are gross,
  # Binance's executed quote is rebuilt from its trades: quote + a USDT commission)].
  def venue_limit_kit(venue)
    case venue
    when 'kraken'
      order = lambda do |id, status, qty, exec, cost, price, limit: false, fee: '0'|
        { '/0/private/QueryOrders' => [query_body(id => raw_order(status:, vol: limit ? qty : (exec == '0' ? '60' : qty), vol_exec: exec, cost:, price:,
                                                                   viqc: false, limit_price: limit ? '50000' : '0').merge('fee' => fee))] }
      end
      { 'key' => 'http', 'ticker' => VENUE_PAIRS['kraken']['BTC'], 'placed' => 'OTX-1', 'ext' => ->(n) { "OK-#{n}" },
        'http' => { '/0/public/Ticker' => [ticker_body('49990.1', '50000.2', '49995.3')], '/0/private/AddOrder' => [{ 'error' => [], 'result' => { 'txid' => ['OTX-1'] } }],
                    '/0/private/BalanceEx' => [balance_body('100000')], '/0/private/QueryOrders' => [query_body({})],
                    '/0/private/TradesHistory' => [{ 'error' => [], 'result' => { 'trades' => {}, 'count' => 0 } }] },
        'resting' => ->(id) { order.(id, 'open', '0.00093', '0', '0', '0', limit: true) },
        'open_market' => ->(id) { order.(id, 'open', '0.0012', '0', '0', '0') },
        'fill' => ->(id, quote, qty, price, fee: '0') { order.(id, 'closed', qty, qty, quote, price, fee:) },
        'ask' => ->(ask) { { '/0/public/Ticker' => [ticker_body('49990.1', ask, '49995.3')] } } }
    when 'binance'
      order = lambda do |id, status, type: 'MARKET', executed: '0.00000000', cumulative: '0.00000000', trades: []|
        n = id.delete_prefix('BTCUSDT-').to_i
        { 'GET /api/v3/allOrders' => [ok([bn_order(n, status, type:, executed:, cumulative:)])], 'GET /api/v3/order' => [ok(bn_order(n, status, type:, executed:, cumulative:))],
          'GET /api/v3/myTrades' => [ok(trades)] }
      end
      { 'key' => 'binance', 'ticker' => VENUE_PAIRS['binance']['BTC'], 'placed' => 'BTCUSDT-28457', 'ext' => ->(n) { "BTCUSDT-#{500 + n}" },
        'http' => { 'GET /api/v3/ticker/bookTicker' => [bn_book('64000.11000000')], 'GET /api/v3/ticker/price' => [bn_last('64000.05000000')],
                    'POST /api/v3/order' => [ok('symbol' => 'BTCUSDT', 'orderId' => 28_457, 'orderListId' => -1, 'clientOrderId' => 'x', 'transactTime' => 1_788_000_000_000)],
                    'GET /api/v3/account' => [bn_account('100000.00000000')], 'GET /api/v3/allOrders' => [ok([])], 'GET /api/v3/myTrades' => [ok([])] },
        'resting' => ->(id) { order.(id, 'NEW', type: 'LIMIT') },
        'open_market' => ->(id) { order.(id, 'NEW') },
        'fill' => lambda do |id, quote, qty, price, fee: '0'|
          n = id.delete_prefix('BTCUSDT-').to_i
          order.(id, 'FILLED', executed: qty, cumulative: quote, trades: [bn_trade(1, n, price, qty, commission: fee, asset: 'USDT')])
        end,
        'ask' => ->(ask) { { 'GET /api/v3/ticker/bookTicker' => [bn_book(ask)] } } }
    else
      order = ->(id, o) { { "GET /api/v1/orders/#{id}" => [kc_ok(o.merge('id' => id))] } }
      { 'key' => 'kucoin', 'ticker' => { 'base_decimals' => 8, 'quote_decimals' => 6, 'price_decimals' => 1, 'minimum_base_size' => '0.00001', 'minimum_quote_size' => '0.1' },
        'placed' => '6716f0a2c1b6d10007a3b1c5', 'ext' => ->(n) { "KC-#{n}" },
        'http' => { 'GET /api/v1/market/orderbook/level2_20' => [kc_book('64000.2')], 'GET /api/v1/market/allTickers' => [kc_tickers('64000.05')],
                    'POST /api/v1/orders' => [kc_ok('orderId' => '6716f0a2c1b6d10007a3b1c5')], 'GET /api/v1/accounts' => [kc_accounts('100000')],
                    'GET /api/v1/fills' => [kc_fills([])] },
        'resting' => ->(id) { order.(id, kc_order(id, active: true, type: 'limit', price: '64000', size: '0.00093', funds: '0')) },
        'open_market' => ->(id) { order.(id, kc_order(id, active: true)) },
        'fill' => ->(id, quote, qty, _price, fee: '0') { order.(id, kc_order(id, active: false, deal_size: qty, deal_funds: quote).merge('fee' => fee)) },
        'ask' => ->(ask) { { 'GET /api/v1/market/orderbook/level2_20' => [kc_book(ask)] } } }
    end
  end

  # One capped scenario on `venue`: the daily 60 BTC bot with its limit switched on at its start.
  def venue_limited(venue, name, at:, limit: 100.0, settings: {}, transient: {}, transactions: [], http: {}, ticker: nil, **extra)
    kit = venue_limit_kit(venue)
    { 'name' => "limit-#{venue}-#{name}", 'venue' => venue, 'interval' => 'day', 'quote_amount' => 60.0, 'started_at' => BASKET_STARTED,
      'settings' => { 'quote_amount_limited' => true, 'quote_amount_limit' => limit }.merge(settings), 'settings_changed_at' => nil,
      'transient' => { 'quote_amount_limit_enabled_at' => LIMIT_STAMP }.merge(transient), 'transactions' => transactions,
      'ticker' => ticker || kit['ticker'], 'members' => nil, 'at' => at, 'script' => { kit['key'] => kit['http'].merge(http) }, 'tick' => true }
      .merge(extra.transform_keys(&:to_s))
  end

  def venue_limit_scenarios
    limit_order = { 'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.0025 }
    %w[kraken binance kucoin].flat_map do |venue|
      kit = venue_limit_kit(venue)
      ext = kit['ext']
      waiting_market = ->(id, quote = '60') { limit_row(id, 0, quote_amount: quote, price: '50000') }
      waiting_limit = ->(id) { limit_row(id, 1, order_type: 1, amount: '0.00093', price: '50000', amount_exec: '0', quote_amount_exec: '0') }
      list = []
      # The tally (10), at the second checkpoint (120 owed).
      tally = {
        'closed' => [[limit_closed(ext.(1), '60')], {}],
        'open_limit' => [[waiting_limit.(ext.(2))], kit['resting'].(ext.(2))],
        'unknown_market' => [[waiting_market.(ext.(3))], kit['open_market'].(ext.(3))],
        'cancelled_partial' => [[limit_row(ext.(4), 3, quote_amount: '60', quote_amount_exec: '20', amount_exec: '0.000311769')], {}],
        'cancelled_unfilled' => [[limit_row(ext.(5), 3, quote_amount: '60', quote_amount_exec: '0', amount_exec: '0')], {}],
        'abandoned' => [[limit_row(ext.(6), 4, quote_amount: '60')], {}],
        'failed' => [[limit_row(nil, nil, status: 1, quote_amount: '60', quote_amount_exec: '0', amount_exec: '0')], {}],
        'skipped' => [[limit_row(nil, nil, status: 2, quote_amount: '0.4', quote_amount_exec: '0', amount_exec: '0')], {}],
        'before_stamp' => [[limit_closed(ext.(7), '100')], {}]
      }
      tally.each do |kind, (rows, http)|
        transient = kind == 'before_stamp' ? { 'quote_amount_limit_enabled_at' => '2026-09-01T12:00:00.000Z' } : {}
        list << venue_limited(venue, "tally-#{kind}", at: basket_at(1), transient:, transactions: rows, http:)
      end
      list << venue_limited(venue, 'tally-closed-limit_order', at: basket_at(1), settings: limit_order, transactions: [limit_closed(ext.(1), '60')])
      # The stop on a fill: found by the follow-up, and by the tick's sweep (Bot::StopJob after the run), each with its mail.
      list << venue_limited(venue, 'poll-reaches', at: basket_at(1), limit: 60.0, tick: false, poll: ext.(8), transactions: [waiting_market.(ext.(8))],
                            http: kit['fill'].(ext.(8), '60', '0.0012', '50000'))
      list << venue_limited(venue, 'sweep-reaches', at: basket_at(1), limit: 60.0, transactions: [waiting_market.(ext.(9))],
                            http: kit['fill'].(ext.(9), '60', '0.0012', '50000'))
      # Fee-inclusive buy totals: a 0.06 quote fee on a 60 fill under a 60.06 cap. Binance's executed quote is 60 + 0.06 (the
      # cap is spent and the bot stops); Kraken's and KuCoin's are the gross 60 (0.06 is left, over the precision floor).
      list << venue_limited(venue, 'fee-quote', at: basket_at(1), limit: 60.06, tick: false, poll: ext.(10), transactions: [waiting_market.(ext.(10))],
                            http: kit['fill'].(ext.(10), '60', '0.0012', '50000', fee: '0.06'))
      # A base-sized market buy that fills above its sized price: the cap is soft there (Rails spends past it, then stops).
      list << venue_limited(venue, 'slippage-base', at: basket_at(1), transactions: [limit_closed(ext.(1), '40')], poll: kit['placed'],
                            ticker: venue == 'binance' ? kit['ticker'].merge('minimum_base_size' => '0.0001') : nil,
                            http: kit['fill'].(kit['placed'], '60.45', '0.00093', '65000'))
      # The exact-cap guard's trim (a listed divergence, rust/tests/parity.rs): 100 - 96.82 (or 92.82) is the Float
      # 3.180000000000007 (7.180000000000007), so Rails sizes 0.0002 BTC at an ask a hair above 5000 × the exact remainder
      # and spends 3e-15 over the cap; Rust removes one base increment.
      spent, ask, ticker = venue == 'binance' ? ['92.82', '35900.000000000015', kit['ticker'].merge('minimum_base_size' => '0.00015')] : ['96.82', '15900.000000000015', nil]
      list << venue_limited(venue, 'guard-trim', at: basket_at(1), ticker:, http: kit['ask'].(ask),
                            transactions: [limit_row(ext.(11), 3, quote_amount: '100', quote_amount_exec: spent, amount_exec: '0.002')])
      list
    end.tap { |l| raise "the venue limit grid has #{l.size} scenarios, not 45" unless l.size == 45 }
  end

  # Plan 2d: Alpaca stocks and ETFs. Tuesday 2026-09-01, 10:00 EDT; September is EDT, so a session is 13:30–20:00 UTC.
  STOCK_STARTED = '2026-09-01T14:00:00.123456Z'

  # As MarketData.sync_stocks_from_deltabadger! and #sync_alpaca_listings_from_deltabadger! import them: category Stock,
  # instrument_type stock, external_id "<SYMBOL>.US", the bare symbol as ticker, MarketData::STOCK_TICKER_DEFAULTS. A
  # composition is saved as BotApi::Bots::Create saves one (refresh_composition writes bot_index_assets).
  def build_stock(dir, sc)
    SchemaTemplate.install(dir)
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
    user = User.new(name: 'Owner', email: 'owner@example.com', encrypted_password: SchemaTemplate.owner_password('correct horse battery staple'), admin: true,
                    confirmed_at: Time.current, setup_completed: true)
    user.save!(validate: false)
    alpaca = Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25')
    usd = Asset.create!(external_id: 'usd', symbol: 'USD', name: 'US Dollar', category: 'Fiat')
    ExchangeAsset.create!(exchange: alpaca, asset: usd, available: true)
    members = sc.fetch('members', [['AAPL', 1.0]])
    assets = members.to_h do |sym, _|
      asset = Asset.create!(external_id: "#{sym}.US", symbol: sym, name: sym, category: 'Stock', instrument_type: 'stock')
      ExchangeAsset.create!(exchange: alpaca, asset:, available: true)
      Ticker.create!(exchange: alpaca, ticker: sym, base: sym, quote: 'USD', base_asset: asset, quote_asset: usd,
                     **MarketData::STOCK_TICKER_DEFAULTS.transform_keys(&:to_sym))
      [sym, asset]
    end
    ApiKey.new(user:, exchange: alpaca, key: 'k', secret: 's', passphrase: 'paper', status: :correct, key_type: :trading).save!(validate: false)
    bot = user.bots.new(type: 'Bots::DcaMultiAsset', exchange: alpaca, settings: {
      'quote_asset_id' => usd.id, 'quote_amount' => sc['quote_amount'], 'interval' => sc['interval'], 'weighting' => 'manual',
      'allocations' => members.to_h { |sym, w| [assets.fetch(sym).id.to_s, w] }
    }.merge(sc['settings']))
    bot.set_missed_quote_amount
    bot.save!
    bot.update_columns(status: Bot.statuses[:scheduled], settings_changed_at: nil, started_at: Time.iso8601(sc['started_at']),
                       transient_data: bot.reload.transient_data.merge(sc['transient']))
    sc['transactions'].each do |t|
      sym = t.fetch('symbol', members.first.first)
      Transaction.insert!(t.except('symbol', 'foreign').merge('bot_id' => bot.id, 'exchange_id' => t['foreign'] ? foreign_venue.id : alpaca.id,
                                                   'base_asset_id' => assets.fetch(sym).id,
                                                   'quote_asset_id' => usd.id, 'base' => sym, 'quote' => 'USD', 'side' => 0, 'transaction_type' => 'REGULAR',
                                                   'bot_interval' => sc['interval'], 'bot_quote_amount' => sc['quote_amount'], 'error_messages' => [],
                                                   'updated_at' => t['created_at']))
    end
    # As the account sync books a split (Exchanges::Alpaca#normalize_split, #merge_split_entries).
    sc.fetch('splits', []).each do |s|
      raw = { 'activity_type' => 'SPLIT', 'symbol' => s['name'], 'corporate_action' => 'split' }
      net = 0
      if s['ratio']
        raw['split_ratio'] = s['ratio']
        p, q = s['ratio'].split(':').map(&:to_d)
        raw['qty'] = '-0.32'
        raw['merged_activity_ids'] = ['remove', 'add']
        net = 0.32.to_d * (p / q - 1)
      end
      AccountTransaction.insert!({ 'user_id' => user.id, 'exchange_id' => alpaca.id, 'entry_type' => 15, 'base_currency' => s['name'], 'base_amount' => net,
                                   'transacted_at' => s['at'], 'raw_data' => raw, 'created_at' => s['at'], 'updated_at' => s['at'] })
    end
    unless sc.fetch('splits', []).empty?
      positions = assets.filter_map do |sym, asset|
        held = bot.transactions.submitted.where(base_asset_id: asset.id).sum(:amount_exec).to_d
        sc.fetch('splits', []).select { |sp| sp['name'] == sym && sp['ratio'] }.each do |sp|
          p,q = sp['ratio'].split(':').map(&:to_d); held *= p/q
        end
        { 'symbol' => sym, 'asset_class' => 'us_equity', 'qty' => held.to_s('F') } if held.positive?
      end
      sc['script']['alpaca']['GET /v2/positions'] = [ok(positions)]
    end
    bot
  end

  # /v2/clock as Alpaca documents it, for the session of `at` (a weekday in EDT).
  def clock_body(open, next_open, next_close, at) = { 'timestamp' => at, 'is_open' => open, 'next_open' => next_open, 'next_close' => next_close }
  def session_day(at) = Time.iso8601(at).utc.to_date
  def open_clock(at) = ok(clock_body(true, "#{session_day(at) + 1}T09:30:00-04:00", "#{session_day(at)}T16:00:00-04:00", at))
  def closed_clock(at) = ok(clock_body(false, "#{session_day(at) + 1}T09:30:00-04:00", "#{session_day(at) + 1}T16:00:00-04:00", at))
  def stock_quote(sym, ask) = ok('symbol' => sym, 'quote' => { 'ap' => ask, 'as' => 100, 'bp' => ask, 'bs' => 100, 't' => '2026-09-01T14:00:00Z' })
  def stock_trade(sym, price) = ok('symbol' => sym, 'trade' => { 'p' => price, 's' => 10, 't' => '2026-09-01T14:00:00Z' })
  def stock_account(cash, buying_power) = ok('id' => 'paper-account', 'status' => 'ACTIVE', 'currency' => 'USD', 'cash' => cash,
                                             'buying_power' => buying_power, 'non_marginable_buying_power' => cash)
  def stock_order(id, status, sym: 'AAPL', type: 'market', notional: '60', qty: nil, filled_qty: '0', filled_avg_price: nil, limit_price: nil)
    { 'id' => id, 'client_order_id' => "rails-#{id}", 'symbol' => sym, 'asset_class' => 'us_equity', 'notional' => notional, 'qty' => qty,
      'filled_qty' => filled_qty, 'filled_avg_price' => filled_avg_price, 'order_type' => type, 'type' => type, 'side' => 'buy',
      'time_in_force' => 'day', 'limit_price' => limit_price, 'status' => status }
  end

  def stock_scenarios
    started = STOCK_STARTED
    modes = { 'market' => {}, 'limit' => { 'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.0025 } }
    closed = { 'status' => 0, 'external_status' => 2, 'external_id' => 'OCLOSED-1', 'order_type' => 0, 'quote_amount' => '60',
               'quote_amount_exec' => '60', 'amount_exec' => '0.32', 'price' => '187.5', 'created_at' => '2026-09-01 14:00:01' }
    waiting = { 'status' => 0, 'external_status' => 0, 'external_id' => 'OOPEN-1', 'order_type' => 0, 'quote_amount' => '60', 'price' => '187.5',
                'amount_exec' => '0', 'quote_amount_exec' => '0', 'created_at' => '2026-09-01 14:00:01' }
    # A position bought before the start, so a split can be settled (more than two days old) at the first checkpoint.
    held = closed.merge('external_id' => 'OCLOSED-S', 'created_at' => '2026-08-25 14:00:01')
    %w[day week].flat_map do |interval|
      step = { 'day' => 1.day, 'week' => 1.week }.fetch(interval)
      at_n = ->(n, utc_hour = 14, sec = 1) { (Time.iso8601(started).utc.beginning_of_day + (n * step) + utc_hour.hours + sec).iso8601(6) }
      at1 = at_n.(1)
      after_close = at_n.(1, 20, 10) # 16:00:10 EDT
      day1 = session_day(at1)
      stale_open = clock_body(true, "#{day1 + 1}T09:30:00-04:00", "#{day1}T16:00:00-04:00", after_close)
      one_day_before = (Time.iso8601(at1) - 1.day).utc.strftime('%Y-%m-%d %H:%M:%S')
      order_answer = ->(status, **kw) { { 'GET /v2/orders/OOPEN-1' => [ok(stock_order('OOPEN-1', status, **kw))] } }
      variants = {
        'first_tick' => { 'at' => (Time.iso8601(started) + 0.5).iso8601(6) },
        'on_schedule' => { 'at' => at1, 'transactions' => [closed] },
        'closed' => { 'at' => at1, 'http' => { 'GET /v2/clock' => [closed_clock(at1)] } },
        'closed_again' => { 'at' => at1, 'transient' => { 'waiting_for_market_open' => true }, 'http' => { 'GET /v2/clock' => [closed_clock(at1)] } },
        'open_after_park' => { 'at' => at_n.(3), 'transient' => { 'waiting_for_market_open' => true } },
        'clock_timeout' => { 'at' => at1, 'http' => { 'GET /v2/clock' => [POST_SEND] } },
        'clock_401' => { 'at' => at1, 'http' => { 'GET /v2/clock' => [{ 'status' => 401, 'body' => { 'code' => 40_110_000, 'message' => 'unauthorized.' } }] } },
        # Listed divergences (STOCK_DIVERGENCES): Rails reads each as open and places a day order; Rust places nothing.
        'clock_5xx' => { 'at' => at1, 'http' => { 'GET /v2/clock' => [{ 'status' => 503, 'body' => { 'code' => 50_310_000, 'message' => 'service unavailable' } }] } },
        'clock_unreadable' => { 'at' => at1, 'http' => { 'GET /v2/clock' => [{ 'status' => 200, 'body' => 'upstream connect error' }] } },
        'clock_certificate' => { 'at' => at1, 'http' => { 'GET /v2/clock' => [CERTIFICATE] } },
        'clock_stale_body' => { 'at' => after_close, 'http' => { 'GET /v2/clock' => [ok(stale_open)] } },
        'clock_stale_cache' => { 'at' => after_close, 'http' => { 'GET /v2/clock' => [closed_clock(after_close)] },
                                 'rails_cache' => { 'at' => (Time.iso8601(after_close) - 30).iso8601(6), 'clock' => stale_open } },
        'clock_past_next_open' => { 'at' => at1, 'http' => { 'GET /v2/clock' => [ok(clock_body(false, "#{day1}T09:30:00-04:00", "#{day1}T16:00:00-04:00", at1))] } },
        'ask_zero' => { 'at' => at1, 'http' => { 'GET /v2/stocks/AAPL/quotes/latest' => [stock_quote('AAPL', 0)], 'GET /v2/stocks/AAPL/trades/latest' => [stock_trade('AAPL', 0)] } },
        'accepted' => { 'at' => at1, 'poll' => 'OTX-1', 'http' => { 'POST /v2/orders' => [ok(stock_order('OTX-1', 'accepted'))],
                                                                    'GET /v2/orders/OTX-1' => [ok(stock_order('OTX-1', 'accepted'))] } },
        'not_fractionable' => { 'at' => at1, 'http' => { 'POST /v2/orders' => [{ 'status' => 403, 'body' => { 'code' => 40_310_000, 'message' => 'asset "AAPL" is not fractionable' } }] } },
        'insufficient_buying_power' => { 'at' => at1, 'http' => { 'POST /v2/orders' => [{ 'status' => 403, 'body' => { 'code' => 40_310_000, 'message' => 'insufficient buying power' } }] } },
        'day_trading' => { 'at' => at1, 'http' => { 'POST /v2/orders' => [{ 'status' => 403, 'body' => { 'code' => 40_310_000, 'message' => 'insufficient day trading buying power' } }] } },
        'add_server_error' => { 'at' => at1, 'http' => { 'POST /v2/orders' => [{ 'status' => 500, 'body' => { 'code' => 50_010_000, 'message' => 'internal server error' } }] } },
        'sweep_expired_unfilled' => { 'at' => at1, 'transactions' => [waiting], 'http' => order_answer.('expired') },
        # Rule "A" (A8): the expired order's fill is spent; only its unfilled part is owed again.
        'sweep_expired_partial' => { 'at' => at1, 'transactions' => [waiting], 'http' => order_answer.('expired', filled_qty: '0.2', filled_avg_price: '187.5') },
        'sweep_done_for_day' => { 'at' => at1, 'transactions' => [waiting], 'http' => order_answer.('done_for_day', filled_qty: '0.1', filled_avg_price: '187.5') },
        'sweep_held' => { 'at' => at1, 'transactions' => [waiting], 'http' => order_answer.('held') },
        'sweep_accepted' => { 'at' => at1, 'transactions' => [waiting], 'http' => order_answer.('accepted') },
        'poll_expired_partial' => { 'at' => at1, 'tick' => false, 'poll' => 'OOPEN-1', 'transactions' => [waiting],
                                    'http' => order_answer.('expired', filled_qty: '0.2', filled_avg_price: '187.5') },
        # Cash and buying power on opposite sides of the buffer: a stock bot spends buying_power (#spendable_balance).
        'funds_low_buying_power' => { 'at' => at1, 'http' => { 'GET /v2/account' => [stock_account('100000', '1')] } },
        'funds_low_cash_only' => { 'at' => at1, 'http' => { 'GET /v2/account' => [stock_account('1', '100000')] } },
        'split_recent' => { 'at' => at1, 'transactions' => [held], 'splits' => [{ 'name' => 'AAPL', 'at' => one_day_before, 'ratio' => '4:1' }] },
        'split_settled' => { 'at' => at1, 'transactions' => [held], 'splits' => [{ 'name' => 'AAPL', 'at' => '2026-08-28 00:00:00', 'ratio' => '4:1' }] },
        'split_unresolved' => { 'at' => at1, 'transactions' => [held], 'splits' => [{ 'name' => 'AAPL', 'at' => '2026-08-28 00:00:00', 'ratio' => nil }] },
        'basket_split_settled' => { 'at' => at1, 'members' => [['AAPL', 0.5], ['MSFT', 0.5]],
                                    'transactions' => [held, held.merge('external_id' => 'OCLOSED-M', 'symbol' => 'MSFT', 'price' => '400', 'amount_exec' => '0.15')],
                                    'splits' => [{ 'name' => 'AAPL', 'at' => '2026-08-28 00:00:00', 'ratio' => '4:1' }] }
      }
      raise "the stock grid has #{variants.size} variants, not 31" unless variants.size == 31

      modes.flat_map do |mode, settings|
        variants.map do |name, v|
          base_http = { 'GET /v2/clock' => [open_clock(v.fetch('at'))],
                        'GET /v2/stocks/AAPL/quotes/latest' => [stock_quote('AAPL', 187.43)], 'GET /v2/stocks/AAPL/trades/latest' => [stock_trade('AAPL', 187.41)],
                        'GET /v2/stocks/MSFT/quotes/latest' => [stock_quote('MSFT', 401.25)], 'GET /v2/stocks/MSFT/trades/latest' => [stock_trade('MSFT', 401.2)],
                        'POST /v2/orders' => [ok(stock_order('OTX-1', 'new')), ok(stock_order('OTX-2', 'new', sym: 'MSFT'))],
                        'GET /v2/account' => [stock_account('100000', '200000')], 'GET /v2/positions' => [ok([])] }
          { 'name' => "#{interval}-#{mode}-#{name}", 'venue' => 'alpaca', 'asset' => 'stock', 'interval' => interval, 'quote_amount' => 60.0,
            'started_at' => started, 'settings' => settings, 'transient' => v.fetch('transient', {}), 'transactions' => v.fetch('transactions', []),
            'members' => v['members'], 'splits' => v.fetch('splits', []), 'at' => v.fetch('at'), 'script' => { 'alpaca' => base_http.merge(v.fetch('http', {})) },
            'rails_cache' => v['rails_cache'], 'tick' => v.fetch('tick', true), 'poll' => v['poll'] }.compact
        end
      end
    end
  end

  DATA_PRE_SEND = { 'network' => 'pre_send_data', 'message' => 'Faraday::ConnectionFailed: Connection refused - connect(2) for "data.alpaca.markets" port 443' }.freeze

  INDEX_SYMBOLS = %w[AAA BBB CCC DDD EEE].freeze

  # An ND100-shaped universe: five stocks as the data-api sync imports them, data-api's 'nasdaq-100' row (top_coins in ranking
  # order, weights = the vendor caps; stock assets carry no market_cap, so get_top_coins reads the weights), and a weekly
  # Bots::DcaIndex over it. Members and holdings are seeded as earlier ticks left them.
  def build_index(dir, sc)
    SchemaTemplate.install(dir)
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
    user = User.new(name: 'Owner', email: 'owner@example.com', encrypted_password: SchemaTemplate.owner_password('correct horse battery staple'), admin: true,
                    confirmed_at: Time.current, setup_completed: true)
    user.save!(validate: false)
    alpaca = Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25')
    usd = Asset.create!(external_id: 'usd', symbol: 'USD', name: 'US Dollar', category: 'Fiat')
    ExchangeAsset.create!(exchange: alpaca, asset: usd, available: true)
    tickers = (INDEX_SYMBOLS + ['ZZZ']).to_h do |sym|
      asset = Asset.create!(external_id: "#{sym}.US", symbol: sym, name: sym, category: 'Stock', instrument_type: 'stock')
      ExchangeAsset.create!(exchange: alpaca, asset: asset, available: true)
      next [sym, nil] if sym == 'ZZZ' # ranked by data-api, never listed on Alpaca (entrant_no_ticker)

      [sym, Ticker.create!(exchange: alpaca, ticker: sym, base: sym, quote: 'USD', base_asset: asset, quote_asset: usd,
                           **MarketData::STOCK_TICKER_DEFAULTS.transform_keys(&:to_sym))]
    end
    AppConfig.market_data_provider = 'deltabadger'
    AppConfig.market_data_url = 'https://data.invalid'
    AppConfig.market_data_token = 'scripted-token'
    unless sc['index_missing']
      Index.create!(external_id: 'nasdaq-100', source: 'deltabadger', name: 'Nasdaq 100', top_coins: sc['top_coins'].map { |s| "#{s}.US" },
                    weights: sc.fetch('weights', { 'AAA.US' => 5e12, 'BBB.US' => 3e12, 'CCC.US' => 2e12, 'DDD.US' => 1e12, 'EEE.US' => 5e11, 'ZZZ.US' => 4e12 }),
                    top_coins_by_exchange: {}, available_exchanges: {}, weight: 0)
    end
    ApiKey.new(user:, exchange: alpaca, key: 'k', secret: 's', passphrase: 'paper', status: :correct, key_type: :trading).save!(validate: false)
    bot = Bots::DcaIndex.new(user:, exchange: alpaca, settings: {
      'quote_asset_id' => usd.id, 'quote_amount' => 60.0, 'interval' => 'week', 'index_type' => 'category', 'index_category_id' => 'nasdaq-100',
      'num_coins' => sc['num_coins'], 'allocation_flattening' => sc['flattening'], 'hold_all' => sc['hold_all']
    }.merge(sc['settings'] || {}))
    bot.set_missed_quote_amount
    bot.save!(validate: false)
    bot.update_columns({ status: Bot.statuses[:scheduled], settings_changed_at: nil, label: 'ND100', started_at: Time.iso8601(STOCK_STARTED) }
                       .merge((sc['bot_columns'] || {}).transform_keys(&:to_sym)))
    sc['members'].each do |m|
      BotIndexAsset.insert!({ 'bot_id' => bot.id, 'asset_id' => tickers.fetch(m['symbol']).base_asset_id, 'ticker_id' => tickers.fetch(m['symbol']).id,
                              'target_allocation' => m['target'], 'in_index' => m['in_index'], 'entered_at' => '2026-08-01 00:00:00',
                              'exited_at' => m['in_index'] ? nil : '2026-08-15 10:30:00', 'created_at' => '2026-08-01 00:00:00', 'updated_at' => '2026-08-01 00:00:00' })
    end
    sc['transactions'].each_with_index do |(sym, amount), i|
      t = tickers.fetch(sym)
      Transaction.insert!({ 'bot_id' => bot.id, 'exchange_id' => alpaca.id, 'external_id' => "OHELD-#{i}", 'status' => 0, 'external_status' => 2,
                            'side' => 0, 'order_type' => 0, 'amount' => amount, 'price' => '100', 'amount_exec' => amount,
                            'quote_amount' => (BigDecimal(amount) * 100).to_s('F'), 'quote_amount_exec' => (BigDecimal(amount) * 100).to_s('F'),
                            'base' => sym, 'quote' => 'USD', 'base_asset_id' => t.base_asset_id, 'quote_asset_id' => usd.id, 'transaction_type' => 'REGULAR',
                            'bot_interval' => 'week', 'bot_quote_amount' => 60, 'error_messages' => [], 'created_at' => '2026-08-25 14:00:01',
                            'updated_at' => '2026-08-25 14:00:01' })
    end
    Ticker.where(ticker: sc['unavailable']).update_all(available: false) if sc['unavailable']
    bot
  end

  def index_scenarios
    at = (Time.iso8601(STOCK_STARTED) + 1.week + 1).iso8601(6)
    asks = INDEX_SYMBOLS.zip([100, 200, 300, 400, 500]).to_h
    top3 = INDEX_SYMBOLS.first(3)
    member = ->(sym, target, in_index: true) { { 'symbol' => sym, 'target' => target, 'in_index' => in_index } }
    steady = [member.('AAA', 0.5), member.('BBB', 0.3), member.('CCC', 0.2)]
    compositions = {
      'duplicate' => { 'members' => [], 'top_coins' => %w[AAA AAA BBB], 'weights' => { 'AAA.US' => 100, 'BBB.US' => 10 } },
      'steady' => { 'members' => steady },
      'fresh' => { 'members' => [] },
      'leaver' => { 'members' => steady, 'top_coins' => %w[AAA BBB DDD EEE] },
      'reentrant' => { 'members' => [member.('AAA', 0.5), member.('BBB', 0.3), member.('CCC', 0.2, in_index: false)] },
      'entrant_unpriced' => { 'members' => [], 'asks' => { 'CCC' => 0 } },
      'entrant_probe_transient' => { 'members' => [], 'network' => 'CCC' },
      'incumbent_unpriced' => { 'members' => steady, 'asks' => { 'CCC' => 0 } },
      'entrant_no_ticker' => { 'members' => [], 'top_coins' => %w[AAA ZZZ BBB CCC DDD EEE] },
      'member_unavailable' => { 'members' => steady, 'unavailable' => ['CCC'] },
      'index_missing' => { 'members' => steady, 'index_missing' => true }
    }
    holdings = { 'none' => [], 'at_target' => [%w[AAA 0.5], %w[BBB 0.3], %w[CCC 0.2]], 'drifted' => [%w[AAA 1.0], %w[BBB 0.3], %w[CCC 0.2]] }
    weightings = { 'top3' => { 'num_coins' => 3, 'flattening' => 0.0, 'hold_all' => false },
                   'all_flat' => { 'num_coins' => 3, 'flattening' => 0.5, 'hold_all' => true } }
    compositions.flat_map do |cname, comp|
      holdings.flat_map do |hname, rows|
        weightings.map do |wname, w|
          prices = asks.merge(comp.fetch('asks', {}))
          http = { 'GET /v2/clock' => [open_clock(at)], 'GET /v2/account' => [stock_account('100000', '200000')], 'GET /v2/positions' => [ok([])],
                   'POST /v2/orders' => (1..5).map { |n| ok(stock_order("OTX-#{n}", 'new')) } }
          prices.each { |sym, ask| http["GET /v2/stocks/#{sym}/quotes/latest"] = [stock_quote(sym, ask)] }
          http["GET /v2/stocks/#{comp['network']}/quotes/latest"] = [DATA_PRE_SEND] if comp['network']
          { 'name' => "index-#{cname}-#{hname}-#{wname}", 'venue' => 'alpaca', 'kind' => 'index', 'at' => at,
            'top_coins' => comp.fetch('top_coins', INDEX_SYMBOLS), 'members' => comp['members'], 'transactions' => rows,
            'weights' => comp['weights'], 'unavailable' => comp['unavailable'], 'index_missing' => comp['index_missing'], 'script' => { 'alpaca' => http }, 'tick' => true }
            .merge(w).compact
        end
      end
    end.tap { |list| raise "the index grid has #{list.size} scenarios, not 66" unless list.size == 66 }
  end

  # Plan CI (T4): crypto index bots (Top coins and a CoinGecko category) on Kraken (EUR), Binance (USDT, and BTC: a quote
  # that is itself a coin), and Alpaca (USD crypto beside USD stocks), on a data-api install, every venue scripted at the
  # Faraday adapter. The assets are as the data-api asset sync imports them (category Cryptocurrency, market_cap set); the
  # two indices as the syncs store them: `top-coins` (the image's seed, source internal) and `layer-1` (source coingecko),
  # neither with weights.
  VI_COINS = { 'bitcoin' => ['BTC', 1.3e12], 'ethereum' => ['ETH', 4e11], 'tether' => ['USDT', 1.2e11], 'usd-coin' => ['USDC', 6e10],
               'solana' => ['SOL', 8e10], 'cardano' => ['ADA', 2e10], 'avalanche-2' => ['AVAX', 1e10] }.freeze
  VI_TOP = %w[bitcoin ethereum tether usd-coin solana cardano avalanche-2].freeze
  VI_L1 = %w[bitcoin ethereum solana cardano avalanche-2].freeze
  VI_QUOTE = { 'kraken' => 'EUR', 'binance' => 'USDT', 'binance_btc' => 'BTC', 'kucoin' => 'USDT', 'kucoin_usdc' => 'USDC', 'alpaca' => 'USD' }.freeze
  KUCOIN_VENUES = %w[kucoin kucoin_usdc].freeze
  # [ask, last] per base listed at the quote (bid = last).
  VI_PRICES = {
    'EUR' => { 'BTC' => %w[60000.2 59990.1], 'ETH' => %w[2300.25 2300.01], 'USDT' => %w[0.9201 0.92], 'USDC' => %w[0.9202 0.9201],
               'SOL' => %w[140.125 140.001], 'ADA' => %w[0.40012 0.4001], 'AVAX' => %w[25.012 25.001] },
    'USDT' => { 'BTC' => %w[64000.11 64000.05], 'ETH' => %w[2500.51 2500.47], 'USDC' => %w[1.0001 1.0], 'SOL' => %w[150.125 150.123],
                'ADA' => %w[0.4501 0.45], 'AVAX' => %w[27.01 27.0] },
    'USDC' => { 'BTC' => %w[64001.12 64001.05], 'ETH' => %w[2500.71 2500.6], 'USDT' => %w[0.9999 0.9998], 'SOL' => %w[150.13 150.12],
                'ADA' => %w[0.4502 0.4501], 'AVAX' => %w[27.02 27.01] },
    'BTC' => { 'ETH' => %w[0.03901 0.039], 'SOL' => %w[0.002301 0.0023], 'ADA' => %w[0.00000701 0.000007], 'AVAX' => %w[0.000401 0.0004] },
    'USD' => { 'BTC' => %w[64000.0 63990.0], 'ETH' => %w[2500.0 2499.5], 'USDT' => %w[1.0001 1.0], 'USDC' => %w[1.0002 1.0001], 'SOL' => %w[150.0 149.9] }
  }.freeze
  VI_STARTED = '2026-09-01T10:00:00.123456Z'

  def vi_ticker(venue, sym)
    quote = VI_QUOTE.fetch(venue)
    case venue
    when 'kraken' then sym == 'BTC' ? 'XBTEUR' : "#{sym}EUR"
    when 'alpaca' then "#{sym}/USD"
    when *KUCOIN_VENUES then "#{sym}-#{quote}"
    else "#{sym}#{quote}"
    end
  end

  def vi_pair(venue, sym)
    price_decimals = VI_PRICES.fetch(VI_QUOTE.fetch(venue)).fetch(sym)[0].split('.').last.size
    case venue
    when 'kraken' then { 'base_decimals' => 8, 'quote_decimals' => 5, 'price_decimals' => price_decimals, 'minimum_base_size' => '0.00001', 'minimum_quote_size' => '0.5' }
    when 'binance' then { 'base_decimals' => 5, 'quote_decimals' => 8, 'price_decimals' => price_decimals, 'minimum_base_size' => '0.00001', 'minimum_quote_size' => '5' }
    when 'binance_btc' then { 'base_decimals' => 4, 'quote_decimals' => 8, 'price_decimals' => price_decimals, 'minimum_base_size' => '0.0001', 'minimum_quote_size' => '0.0001' }
    # As Exchanges::Kucoin#get_tickers_info stores a spot pair: baseIncrement/quoteIncrement decimals, baseMinSize, quoteMinSize.
    when *KUCOIN_VENUES then { 'base_decimals' => 8, 'quote_decimals' => 6, 'price_decimals' => price_decimals, 'minimum_base_size' => '0.00001', 'minimum_quote_size' => '0.1' }
    else { 'base_decimals' => 9, 'quote_decimals' => 2, 'price_decimals' => price_decimals, 'minimum_base_size' => '0.000001', 'minimum_quote_size' => '1' }
    end
  end

  def build_venue_index(dir, sc)
    SchemaTemplate.install(dir)
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
    user = User.new(name: 'Owner', email: 'owner@example.com', encrypted_password: SchemaTemplate.owner_password('correct horse battery staple'), admin: true,
                    confirmed_at: Time.current, setup_completed: true)
    user.save!(validate: false)
    venue = sc['venue_key']
    exchange = case venue
               when 'kraken' then Exchanges::Kraken.create!(name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4')
               when 'alpaca' then Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25')
               when *KUCOIN_VENUES then Exchanges::Kucoin.create!(name: 'KuCoin', maker_fee: '0.1', taker_fee: '0.1')
               else Exchanges::Binance.create!(name: 'Binance', maker_fee: '0.1', taker_fee: '0.1')
               end
    assets = VI_COINS.to_h do |ext, (sym, cap)|
      [sym, Asset.create!(external_id: ext, symbol: sym, name: sym, category: 'Cryptocurrency', market_cap: (sc['zero_cap'] || []).include?(sym) ? 0 : cap)]
    end
    quote = assets[VI_QUOTE.fetch(venue)] ||
            Asset.create!(external_id: venue == 'kraken' ? 'EUR.FOREX' : 'usd', symbol: VI_QUOTE.fetch(venue), name: VI_QUOTE.fetch(venue), category: 'Currency')
    ([quote] + assets.values).uniq.each { |a| ExchangeAsset.create!(exchange:, asset: a, available: true) }
    tickers = VI_PRICES.fetch(quote.symbol).keys.reject { |sym| (sc['unlisted'] || []).include?(sym) }.to_h do |sym|
      base = venue == 'kraken' && sym == 'BTC' ? 'XBT' : sym
      [sym, Ticker.create!(exchange:, ticker: vi_ticker(venue, sym), base:, quote: quote.symbol, base_asset: assets.fetch(sym), quote_asset: quote,
                           **vi_pair(venue, sym).symbolize_keys)]
    end
    # A venue switch (`former_venue`): the members and buys the bot left on the venue it was moved off, every coin listed there
    # at the same quote; the bot itself is on `venue_key` (the settings save that moved it is behind it).
    former = sc['former_venue'] && Exchanges::Binance.create!(name: 'Binance', maker_fee: '0.1', taker_fee: '0.1') # the one former venue the grid moves off
    ([quote] + assets.values).uniq.each { |a| ExchangeAsset.create!(exchange: former, asset: a, available: true) } if former
    former_tickers = former && VI_PRICES.fetch(quote.symbol).keys.to_h do |sym|
      [sym, Ticker.create!(exchange: former, ticker: vi_ticker(sc['former_venue'], sym), base: sym, quote: quote.symbol, base_asset: assets.fetch(sym), quote_asset: quote,
                           **vi_pair(sc['former_venue'], sym).symbolize_keys)]
    end
    # Alpaca's catalog lists USD stocks too (data-api's listings sync), so a Top crypto bot is not all crypto: its tick reads the clock.
    if venue == 'alpaca'
      %w[AAA BBB].each do |sym|
        stock = Asset.create!(external_id: "#{sym}.US", symbol: sym, name: sym, category: 'Stock', instrument_type: 'stock')
        ExchangeAsset.create!(exchange:, asset: stock, available: true)
        Ticker.create!(exchange:, ticker: sym, base: sym, quote: 'USD', base_asset: stock, quote_asset: quote, **MarketData::STOCK_TICKER_DEFAULTS.transform_keys(&:to_sym))
      end
    end
    AppConfig.market_data_provider = 'deltabadger'
    AppConfig.market_data_url = 'https://data.invalid'
    AppConfig.market_data_token = 'scripted-token'
    unless sc['index_missing']
      Index.create!(external_id: 'top-coins', source: 'internal', name: 'Top coins', top_coins: sc['top'] || VI_TOP, weights: sc['weights'] || {},
                    top_coins_by_exchange: {}, available_exchanges: {}, weight: 0)
      Index.create!(external_id: 'layer-1', source: 'coingecko', name: 'Layer 1', top_coins: sc['l1'] || VI_L1, weights: sc['weights'] || {},
                    top_coins_by_exchange: {}, available_exchanges: {}, weight: 0)
    end
    passphrase = { 'alpaca' => 'paper', 'kucoin' => 'p', 'kucoin_usdc' => 'p' }[venue]
    ApiKey.new(user:, exchange:, key: 'k', secret: 's', passphrase:, status: :correct, key_type: :trading).save!(validate: false)
    bot = Bots::DcaIndex.new(user:, exchange:, settings: {
      'quote_asset_id' => quote.id, 'quote_amount' => sc['quote_amount'], 'interval' => 'day', 'index_type' => sc['index_type'],
      'index_category_id' => sc['index_type'] == 'category' ? 'layer-1' : nil, 'num_coins' => sc['num_coins'], 'allocation_flattening' => sc['flattening'],
      'hold_all' => false
    }.compact.merge(sc['settings'] || {}))
    bot.set_missed_quote_amount
    bot.save!(validate: false)
    bot.update_columns({ status: Bot.statuses[sc.fetch('status', 'scheduled')], settings_changed_at: nil, label: 'Crypto index',
                         started_at: Time.iso8601(VI_STARTED), transient_data: bot.reload.transient_data.merge(sc['transient'] || {}) })
    (sc['members'] || []).each do |sym, target, in_index|
      t = (former_tickers || tickers).fetch(sym)
      BotIndexAsset.insert!({ 'bot_id' => bot.id, 'asset_id' => t.base_asset_id, 'ticker_id' => t.id, 'target_allocation' => target, 'in_index' => in_index != false,
                              'entered_at' => '2026-08-01 00:00:00', 'exited_at' => in_index == false ? '2026-08-15 10:30:00' : nil,
                              'created_at' => '2026-08-01 00:00:00', 'updated_at' => '2026-08-01 00:00:00' })
    end
    (sc['transactions'] || []).each_with_index do |row, i|
      t = (former_tickers || tickers).fetch(row['symbol'])
      Transaction.insert!({ 'bot_id' => bot.id, 'exchange_id' => (former || exchange).id, 'external_id' => row.fetch('external_id', "OHELD-#{i}"), 'status' => 0,
                            'external_status' => row.fetch('external_status', 2), 'side' => 0, 'order_type' => row.fetch('order_type', 0), 'amount' => row['amount'],
                            'price' => row['price'], 'amount_exec' => row['amount_exec'], 'quote_amount' => row['quote_amount'], 'quote_amount_exec' => row['quote_amount_exec'],
                            'base' => t.base_asset.symbol, 'quote' => quote.symbol, 'base_asset_id' => t.base_asset_id, 'quote_asset_id' => quote.id,
                            'transaction_type' => 'REGULAR', 'bot_interval' => 'day', 'bot_quote_amount' => sc['quote_amount'], 'error_messages' => [],
                            'created_at' => '2026-09-01 10:00:01', 'updated_at' => '2026-09-01 10:00:01' })
    end
    Ticker.where(exchange:, base_asset_id: (sc['unavailable'] || []).map { |sym| assets.fetch(sym).id }).update_all(available: false)
    bot
  end

  # One member's closed buy of `value` quote at its ask.
  def vi_held(venue, sym, value)
    ask = VI_PRICES.fetch(VI_QUOTE.fetch(venue)).fetch(sym)[0].to_d
    qty = (value.to_d / ask).round(vi_pair(venue, sym)['base_decimals'])
    { 'symbol' => sym, 'quote_amount' => value.to_d.to_s('F'), 'quote_amount_exec' => (qty * ask).to_s('F'), 'amount_exec' => qty.to_s('F'),
      'amount' => qty.to_s('F'), 'price' => ask.to_s('F') }
  end

  def vi_kraken_ticker(pair, ask, last) = { 'error' => [], 'result' => { pair => {
    'a' => [ask, '1', '1.000'], 'b' => [last, '1', '1.000'], 'c' => [last, '0.001'], 'v' => ['12.5', '30.1'], 'p' => [last, last],
    't' => [100, 250], 'l' => [last, last], 'h' => [ask, ask], 'o' => last } } }

  # The price replies of every listed member, with `prices` overriding one symbol's replies (a list: answered in turn).
  def vi_prices(venue, prices)
    quote = VI_QUOTE.fetch(venue)
    VI_PRICES.fetch(quote).flat_map do |sym, (ask, last)|
      pair = vi_ticker(venue, sym)
      over = prices[sym]
      case venue
      when 'kraken' then [["GET /0/public/Ticker?pair=#{pair}", over || [ok(vi_kraken_ticker(pair, ask, last))]]]
      when 'alpaca' then []
      when *KUCOIN_VENUES then [["GET /api/v1/market/orderbook/level2_20?symbol=#{pair}", over || [kc_book(ask, bid: last)]]]
      else [["GET /api/v3/ticker/bookTicker?symbol=#{pair}", over || [ok('symbol' => pair, 'bidPrice' => last, 'bidQty' => '1.5', 'askPrice' => ask, 'askQty' => '0.2')]],
            ["GET /api/v3/ticker/price?symbol=#{pair}", over || [ok('symbol' => pair, 'price' => last)]]]
      end
    end.to_h.merge(KUCOIN_VENUES.include?(venue) ? { 'GET /api/v1/market/allTickers' => [vi_kucoin_tickers(venue, prices.keys)] } : {})
  end

  # KuCoin's last prices come from one allTickers body (#get_last_price, #get_tickers_prices): an unpriced symbol is left out.
  def vi_kucoin_tickers(venue, unpriced)
    rows = VI_PRICES.fetch(VI_QUOTE.fetch(venue)).reject { |sym, _| unpriced.include?(sym) }.map { |sym, (_ask, last)| { 'symbol' => vi_ticker(venue, sym), 'last' => last } }
    kc_ok('time' => 1_788_000_000_000, 'ticker' => rows)
  end

  def vi_http(venue, prices: {}, open: true, over: {})
    quote = VI_QUOTE.fetch(venue)
    http = case venue
           when 'kraken'
             { 'POST /0/private/AddOrder' => (1..6).map { |n| ok('error' => [], 'result' => { 'txid' => ["OTX-#{n}"] }) },
               'POST /0/private/BalanceEx' => [ok(balance_body('100000'))], 'POST /0/private/QueryOrders' => [ok(query_body({}))],
               'POST /0/private/TradesHistory' => [ok('error' => [], 'result' => { 'trades' => {}, 'count' => 0 })] }
           when 'alpaca'
             listed = VI_PRICES.fetch('USD')
             { 'GET /v2/clock' => [ok(clock_body(open, '2026-09-03T09:30:00-04:00', '2026-09-02T16:00:00-04:00', '2026-09-02T06:00:01-04:00'))],
               'GET /v1beta3/crypto/us/latest/quotes' => prices['quotes'] || [ok('quotes' => listed.to_h { |s, (a, l)| ["#{s}/USD", { 'ap' => a.to_f, 'as' => 0.5, 'bp' => l.to_f, 'bs' => 0.4, 't' => '2026-09-01T10:00:00Z' }] })],
               'GET /v1beta3/crypto/us/latest/trades' => prices['trades'] || [ok('trades' => listed.to_h { |s, (_a, l)| ["#{s}/USD", { 'p' => l.to_f, 's' => 0.01, 't' => '2026-09-01T10:00:00Z', 'i' => 1, 'tks' => 'B' }] })],
               'GET /v2/account' => [account('100000')], 'GET /v2/positions' => [ok([])],
               'POST /v2/orders' => (1..6).map { |n| ok(alpaca_order("OTX-#{n}", 'pending_new')) } }
           when *KUCOIN_VENUES
             { 'POST /api/v1/orders' => (1..6).map { |n| kc_ok('orderId' => "6716f0a2c1b6d10007a3b1c#{n}") },
               'GET /api/v1/accounts' => [kc_ok([{ 'id' => 'q', 'currency' => quote, 'type' => 'trade', 'balance' => '100000', 'available' => '100000', 'holds' => '0' }])],
               'GET /api/v1/fills' => [kc_fills([])] }
           else
             { 'POST /api/v3/order' => (1..6).map { |n| ok('symbol' => "BTC#{quote}", 'orderId' => 28_456 + n, 'orderListId' => -1, 'clientOrderId' => 'x', 'transactTime' => 1_788_000_000_000) },
               'GET /api/v3/account' => [ok('makerCommission' => 10, 'canTrade' => true, 'balances' => [{ 'asset' => quote, 'free' => '100000.00000000', 'locked' => '0' }])],
               'GET /api/v3/allOrders' => [ok([])], 'GET /api/v3/myTrades' => [ok([])],
               # The whole market's last prices (Exchange#get_tickers_prices), as the first-tick notice values the skipped legs.
               'GET /api/v3/ticker/price' => [ok(VI_PRICES.fetch(quote).map { |sym, (_ask, last)| { 'symbol' => vi_ticker(venue, sym), 'price' => last } })] }
           end
    http.merge(vi_prices(venue, prices)).merge(over)
  end

  def venue_index(venue, kind, name, at:, num_coins: 3, flattening: 0.0, quote_amount: nil, prices: {}, open: true, over: {}, **extra)
    script_key = { 'kraken' => 'kraken_http', 'alpaca' => 'alpaca', 'kucoin' => 'kucoin', 'kucoin_usdc' => 'kucoin' }.fetch(venue, 'binance')
    { 'name' => "vi-#{venue}-#{kind}-#{name}", 'kind' => 'venue_index', 'venue' => venue.sub(/_(btc|usdc)\z/, ''), 'venue_key' => venue, 'index_type' => kind,
      'num_coins' => num_coins, 'flattening' => flattening, 'quote_amount' => quote_amount || (venue == 'binance_btc' ? 0.001 : 60.0), 'at' => at,
      'script' => { script_key => vi_http(venue, prices:, open:, over:) }, 'tick' => true }.merge(extra.transform_keys(&:to_s))
  end

  # The listed coins of `kind`'s index at the venue's quote, in its ranking order (the quote's own coin has no pair).
  def vi_ranked(venue, list) = list.map { |ext| VI_COINS.fetch(ext)[0] }.select { |sym| VI_PRICES.fetch(VI_QUOTE.fetch(venue)).key?(sym) }

  # The first three as an earlier tick left them, at their market-cap weights.
  def vi_steady(ranked)
    caps = ranked.first(3).to_h { |sym| [sym, VI_COINS.values.to_h.fetch(sym)] }
    Bot::Composition::Weightable.blend(market_caps: caps, flattening: 0.0).map { |sym, w| [sym, w.round(6)] }
  end

  def vi_unpriced(venue, _sym)
    case venue
    when 'kraken' then ok('error' => ['EQuery:Unknown asset pair'])
    when *KUCOIN_VENUES then ok('code' => '400100', 'msg' => 'symbol not exists')
    else { 'status' => 400, 'body' => { 'code' => -1121, 'msg' => 'Invalid symbol.' } }
    end
  end

  # T3's probe table: a newcomer's price read answered (rejected keys, transport failures, a server error) as Rails reads it.
  def vi_probe_failures(venue)
    html = '<html><head><title>401 Authorization Required</title></head><body>401 Authorization Required</body></html>'
    shared = { '401_empty' => { 'status' => 401, 'body' => '' }, '401_html' => { 'status' => 401, 'body' => html },
               '401_json' => { 'status' => 401, 'body' => { 'msg' => 'nope' } },
               'maybe_sent' => { 'network' => 'post_send', 'message' => 'Faraday::TimeoutError: Net::ReadTimeout' } }
    case venue
    when 'kraken'
      shared.merge('invalid_key' => ok('error' => ['EAPI:Invalid key']),
                   'not_sent' => KRAKEN_NOT_SENT,
                   '5xx' => { 'status' => 503, 'body' => { 'error' => ['EService:Unavailable'] } })
    when 'kucoin'
      shared.merge('invalid_key' => { 'status' => 400, 'body' => { 'code' => '400003', 'msg' => 'KC-API-KEY not exists' } },
                   'not_sent' => VENUES.fetch('kucoin').not_sent,
                   '5xx' => { 'status' => 503, 'body' => { 'code' => '500000', 'msg' => 'Internal Server Error' } })
    else
      shared.merge('invalid_key' => { 'status' => 400, 'body' => { 'code' => -2015, 'msg' => 'Invalid API-key, IP, or permissions for action.' } },
                   'not_sent' => VENUES.fetch('binance').not_sent,
                   '5xx' => { 'status' => 502, 'body' => { 'code' => -1001, 'msg' => 'Internal error; unable to process your request. Please try again.' } })
    end
  end

  def venue_index_scenarios
    first = (Time.iso8601(VI_STARTED) + 0.5).iso8601(6)
    day = ->(n) { (Time.iso8601(VI_STARTED) + n.days + 1).iso8601(6) }
    modes = { 'market' => {}, 'limit' => { 'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.0025 } }
    lists = { 'top' => VI_TOP, 'category' => VI_L1 }
    list = []
    (%w[kraken binance binance_btc] + KUCOIN_VENUES).each do |venue|
      lists.each do |kind, index|
        ranked = vi_ranked(venue, index)
        key = kind == 'top' ? 'top' : 'l1'
        steady = vi_steady(ranked)
        held = ranked.first(3).zip([30.0, 20.0, 10.0]).map { |sym, v| vi_held(venue, sym, venue == 'binance_btc' ? v / 60_000 : v) }
        swapped = index.dup
        a, b = [ranked[2], ranked[3]].map { |sym| index.index(VI_COINS.find { |_, (s, _)| s == sym }[0]) }
        swapped[a], swapped[b] = swapped[b], swapped[a]
        second = ranked[1]
        compositions = {
          'fresh' => {}, 'steady' => { 'members' => steady, 'transactions' => held },
          'leaver' => { 'members' => steady, 'transactions' => held, key => swapped },
          'reentrant' => { 'members' => steady[0..1] + [[steady[2][0], steady[2][1], false]], 'transactions' => held },
          'entrant_unpriced' => { 'prices' => { second => [vi_unpriced(venue, second)] } },
          'incumbent_unpriced' => { 'members' => steady, 'transactions' => held, 'prices' => { ranked[2] => [vi_unpriced(venue, ranked[2])] } },
          'entrant_no_ticker' => { 'unlisted' => [second] }, 'member_unavailable' => { 'members' => steady, 'transactions' => held, 'unavailable' => [ranked[2]] },
          'index_missing' => { 'members' => steady, 'transactions' => held, 'index_missing' => true }, 'fewer_than_n' => { 'num_coins' => 20 },
          'zero_market_cap' => { 'zero_cap' => [second] }, 'duplicate' => { key => [index[0]] + index }
        }
        compositions.each do |cname, comp|
          modes.each do |mode, settings|
            at = comp['members'] ? day.(1) : first
            list << venue_index(venue, kind, "#{cname}-#{mode}", at:, settings:, prices: comp.fetch('prices', {}), **comp.except('prices').transform_keys(&:to_sym))
          end
        end
      end
    end
    # The probe table, on a fresh Top coins bot: the second-ranked newcomer's read fails.
    %w[kraken binance].each do |venue|
      second = vi_ranked(venue, VI_TOP)[1]
      vi_probe_failures(venue).each do |fname, reply|
        modes.each { |mode, settings| list << venue_index(venue, 'top', "probe_#{fname}-#{mode}", at: first, settings:, prices: { second => [reply] }) }
      end
    end
    # KuCoin's (plan K O6): its ask is the newcomer's own order book; a limit probe reads the one allTickers body every
    # member shares, so only the market probe fails one newcomer alone.
    second = vi_ranked('kucoin', VI_TOP)[1]
    vi_probe_failures('kucoin').each { |fname, reply| list << venue_index('kucoin', 'top', "probe_#{fname}-market", at: first, prices: { second => [reply] }) }
    KUCOIN_VENUES.each { |venue| list << venue_index(venue, 'category', 'flattened-market', at: first, flattening: 0.5) }
    # Weights flattened, and legs below the venue minimum (one skipped with its amount carried, then all: the notice).
    { 'kraken' => [8.0, 0.6], 'binance' => [40.0, 6.0], 'binance_btc' => [0.001, 0.0001] }.each do |venue, (one, all)|
      list << venue_index(venue, 'category', 'flattened-market', at: first, flattening: 0.5)
      list << venue_index(venue, 'category', 'one_below-market', at: first, quote_amount: one)
      list << venue_index(venue, 'category', 'all_below-market', at: first, quote_amount: all)
    end
    # Rotation with obligations: the leaver holds a resting limit order (open, then partly filled), and a carry across a change.
    %w[kraken binance].each do |venue|
      ranked = vi_ranked(venue, VI_L1)
      steady = vi_steady(ranked)
      swapped = VI_L1.dup.tap { |l| l[2], l[3] = l[3], l[2] } # solana leaves, cardano enters
      leaver = ranked[2]
      pair = vi_ticker(venue, leaver)
      ask = VI_PRICES.fetch(VI_QUOTE.fetch(venue)).fetch(leaver)[0]
      { 'open' => '0', 'partial' => '0.01' }.each do |state, exec|
        ext = venue == 'kraken' ? 'OLIMIT-1' : "#{pair}-90001"
        resting = { 'symbol' => leaver, 'external_id' => ext, 'external_status' => 1, 'order_type' => 1, 'amount' => '0.1', 'price' => ask, 'quote_amount' => (BigDecimal(ask) * BigDecimal('0.1')).to_s('F') }
        over = if venue == 'kraken'
                 { 'POST /0/private/QueryOrders' => [ok(query_body(ext => { 'status' => 'open', 'vol' => '0.1', 'vol_exec' => exec, 'cost' => (BigDecimal(ask) * BigDecimal(exec)).to_s('F'),
                                                                            'price' => exec == '0' ? '0' : ask, 'oflags' => '',
                                                                            'descr' => { 'pair' => pair, 'type' => 'buy', 'ordertype' => 'limit', 'price' => ask } }))] }
               else
                 executed = BigDecimal(exec)
                 order = { 'symbol' => pair, 'orderId' => 90_001, 'orderListId' => -1, 'clientOrderId' => 'web_90001', 'price' => ask, 'origQty' => '0.1',
                           'executedQty' => executed.to_s('F'), 'cummulativeQuoteQty' => (executed * BigDecimal(ask)).to_s('F'),
                           'status' => executed.zero? ? 'NEW' : 'PARTIALLY_FILLED', 'timeInForce' => 'GTC', 'type' => 'LIMIT', 'side' => 'BUY',
                           'origQuoteOrderQty' => '0.00000000', 'time' => 1_788_000_000_000 }
                 trades = executed.zero? ? [] : [{ 'symbol' => pair, 'id' => 7001, 'orderId' => 90_001, 'orderListId' => -1, 'price' => ask, 'qty' => exec,
                                                    'quoteQty' => (executed * BigDecimal(ask)).to_s('F'), 'commission' => '0', 'commissionAsset' => 'BNB',
                                                    'time' => 1_788_000_000_001, 'isBuyer' => true, 'isMaker' => true, 'isBestMatch' => true }]
                 { 'GET /api/v3/allOrders' => [ok([order])], 'GET /api/v3/myTrades' => [ok(trades)] }
               end
        list << venue_index(venue, 'category', "leaver_#{state}_limit-market", at: day.(1), members: steady, l1: swapped, over:,
                            transactions: ranked.first(2).zip([30.0, 20.0]).map { |sym, v| vi_held(venue, sym, v) } + [resting])
      end
      one = venue == 'kraken' ? 8.0 : 40.0
      list << venue_index(venue, 'category', 'carry_across_rotation-market', at: first, quote_amount: one, next_at: day.(1),
                          between: [%(UPDATE indices SET top_coins = '#{swapped.to_json}' WHERE external_id = 'layer-1')])
    end
    # T3b: a trigger, its resync, then a tick; plain, with the resync's newcomer unpriced at the resync, and at the tick.
    %w[kraken binance].each do |venue|
      ranked = vi_ranked(venue, VI_L1)
      steady = vi_steady(ranked)
      held = ranked.first(3).zip([30.0, 20.0, 10.0]).map { |sym, v| vi_held(venue, sym, v) }
      swapped = VI_L1.dup.tap { |l| l[2], l[3] = l[3], l[2] }
      resync = (Time.iso8601(VI_STARTED) + 3600).iso8601(6)
      sequences = {
        'start_then_tick' => [ranked[1], { 'status' => 'stopped', 'resync_at' => first, 'at' => first,
                                           'trigger' => ["UPDATE bots SET status = 1, started_at = '2026-09-01 10:00:00.123456'"] }],
        'delayed_start_then_tick' => [ranked[1], { 'status' => 'stopped', 'resync_at' => first, 'at' => day.(1),
                                                   'trigger' => ["UPDATE bots SET status = 1, started_at = '2026-09-02 10:00:00.123456'"] }],
        'settings_then_tick' => [ranked[2], { 'num_coins' => 2, 'members' => steady.first(2), 'transactions' => held.first(2), 'resync_at' => resync, 'at' => day.(1),
                                              'trigger' => ["UPDATE bots SET settings = json_set(settings, '$.num_coins', 3)"] }],
        'index_sync_then_tick' => [ranked[3], { 'members' => steady, 'transactions' => held, 'resync_at' => resync, 'at' => day.(1),
                                                'trigger' => [%(UPDATE indices SET top_coins = '#{swapped.to_json}' WHERE external_id = 'layer-1')] }],
        'empty_sync_then_tick' => [ranked[2], { 'members' => steady[0..1] + [[steady[2][0], steady[2][1], false]], 'transactions' => held,
                                                'resync_at' => resync, 'at' => day.(1) }]
      }
      sequences.each do |sname, (newcomer, sc)|
        ok_reply = vi_prices(venue, {}).select { |k, _| k.include?("=#{vi_ticker(venue, newcomer)}") }
        { 'plain' => {}, 'unpriced_at_resync' => [vi_unpriced(venue, newcomer), :ok], 'unpriced_at_tick' => [:ok, vi_unpriced(venue, newcomer)] }.each do |fname, order|
          over = order.is_a?(Array) ? ok_reply.transform_values { |(good)| order.map { |r| r == :ok ? good : r } } : {}
          list << venue_index(venue, 'category', "#{sname}-#{fname}", over:, **sc.transform_keys(&:to_sym))
        end
      end
    end
    # The owner's venue switch: a Layer-1 bot moved off Binance to KuCoin (stopped, settings saved, then started) whose stored
    # members are Binance's, the third (SOL here) unlisted on KuCoin. The resync seats KuCoin's own top three and marks SOL out.
    ranked = vi_ranked('kucoin', VI_L1)
    held = ranked.first(3).zip([30.0, 20.0, 10.0]).map { |sym, v| vi_held('binance', sym, v) }
    list << venue_index('kucoin', 'category', 'venue_switch_then_tick-market', former_venue: 'binance', unlisted: [ranked[2]], members: vi_steady(ranked),
                        transactions: held, status: 'stopped', resync_at: (Time.iso8601(VI_STARTED) + 3600).iso8601(6), at: day.(1),
                        trigger: ["UPDATE bots SET status = 1, started_at = '2026-09-01 10:00:00.123456'"])
    # The resync's own failure kinds, then the tick: a rejected key raises (no retry, nothing written), an Alpaca transport
    # failure is retried (n⁴ + 2 s) and succeeds, or fails three times and the job ends.
    started = ["UPDATE bots SET status = 1, started_at = '2026-09-01 10:00:00.123456'"]
    a_minute = (Time.iso8601(VI_STARTED) + 60).iso8601(6)
    %w[kraken binance].each do |venue|
      eth = vi_ticker(venue, 'ETH')
      rejected = { 'status' => 401, 'body' => { 'msg' => 'nope' } }
      over = vi_prices(venue, {}).select { |k, _| k.include?("=#{eth}") }.transform_values { |(good)| [rejected, good] }
      list << venue_index(venue, 'category', 'start_then_tick-rejected_at_resync', status: 'stopped', resync_at: first, at: a_minute, trigger: started, over:)
    end
    fail_quotes = ->(n) { { 'GET /v1beta3/crypto/us/latest/quotes' => [DATA_PRE_SEND] * n + vi_http('alpaca')['GET /v1beta3/crypto/us/latest/quotes'] } }
    list << venue_index('alpaca', 'top', 'start_then_tick-transient_at_resync', status: 'stopped', resync_at: first, at: a_minute, trigger: started, over: fail_quotes.(1))
    list << venue_index('alpaca', 'top', 'start_then_tick-exhausted_at_resync', status: 'stopped', resync_at: first, at: a_minute, trigger: started, over: fail_quotes.(3))
    # Alpaca: Top coins in USD crypto on a catalog that also lists stocks: its tick reads the clock; the resync (no market gate)
    # probes a crypto newcomer on its ask even with the market closed.
    list << venue_index('alpaca', 'top', 'fresh-open', at: first)
    list << venue_index('alpaca', 'top', 'fresh-closed', at: first, open: false)
    list << venue_index('alpaca', 'top', 'closed_resync', at: first, open: false, resync_at: first)
    # The ND100 stock index's resync with the market closed: the stock newcomer DDD is probed on its last and seated. At the
    # open tick its ask fails and it keeps its seat (an incumbent is never probed): EEE does not replace it.
    stock_at = (Time.iso8601(STOCK_STARTED) + 1.week + 1).iso8601(6)
    stock_resync = (Time.iso8601(STOCK_STARTED) + 2.days).iso8601(6)
    http = { 'GET /v2/clock' => [closed_clock(stock_resync), open_clock(stock_at)], 'GET /v2/account' => [stock_account('100000', '200000')],
             'GET /v2/positions' => [ok([])], 'POST /v2/orders' => (1..5).map { |n| ok(stock_order("OTX-#{n}", 'new')) } }
    INDEX_SYMBOLS.zip([100, 200, 300, 400, 500]).each do |sym, ask|
      http["GET /v2/stocks/#{sym}/quotes/latest"] = [stock_quote(sym, sym == 'DDD' ? 0 : ask)]
      http["GET /v2/stocks/#{sym}/trades/latest"] = [stock_trade(sym, ask)]
    end
    list << { 'name' => 'vi-alpaca-stock-closed_resync', 'venue' => 'alpaca', 'kind' => 'index', 'at' => stock_at, 'resync_at' => stock_resync,
              'top_coins' => %w[AAA BBB DDD EEE CCC], 'transactions' => [], 'num_coins' => 3, 'flattening' => 0.0, 'hold_all' => false,
              'members' => [{ 'symbol' => 'AAA', 'target' => 0.6, 'in_index' => true }, { 'symbol' => 'BBB', 'target' => 0.4, 'in_index' => true }],
              'script' => { 'alpaca' => http }, 'tick' => true }
    list.tap { |l| raise "the venue index grid has #{l.size} scenarios, not 331" unless l.size == 331 }
  end

  # Binance spot (spec E2), one asset on BTCUSDT: Rails' own answers to the bodies below, beneath honeymaker.
  def bn_order(id, status, type: 'MARKET', executed: '0.00000000', cumulative: '0.00000000', price: nil, qty: nil, quote: '60.00000000')
    limit = type == 'LIMIT'
    { 'symbol' => 'BTCUSDT', 'orderId' => id, 'orderListId' => -1, 'clientOrderId' => "web_#{id}", 'price' => price || (limit ? '64000.00000000' : '0.00000000'),
      'origQty' => qty || (limit ? '0.00093000' : '0.00000000'), 'executedQty' => executed, 'cummulativeQuoteQty' => cumulative, 'status' => status,
      'timeInForce' => 'GTC', 'type' => type, 'side' => 'BUY', 'origQuoteOrderQty' => limit ? '0.00000000' : quote, 'time' => 1_788_000_000_000 }
  end
  def bn_trade(id, order, price, qty, commission: '0', asset: 'BNB', buyer: true)
    { 'symbol' => 'BTCUSDT', 'id' => id, 'orderId' => order, 'orderListId' => -1, 'price' => price, 'qty' => qty,
      'quoteQty' => (BigDecimal(price) * BigDecimal(qty)).to_s('F'), 'commission' => commission, 'commissionAsset' => asset,
      'time' => 1_788_000_000_000 + id, 'isBuyer' => buyer, 'isMaker' => false, 'isBestMatch' => true }
  end
  def bn_book(ask) = ok('symbol' => 'BTCUSDT', 'bidPrice' => '64000.10000000', 'bidQty' => '1.5', 'askPrice' => ask, 'askQty' => '0.2')
  def bn_last(price) = ok('symbol' => 'BTCUSDT', 'price' => price)
  def bn_account(usdt) = ok('makerCommission' => 10, 'canTrade' => true, 'balances' => [{ 'asset' => 'BTC', 'free' => '0.5', 'locked' => '0' },
                                                                                       { 'asset' => 'USDT', 'free' => usdt, 'locked' => '0' }])
  def bn_fail(status, code, msg) = { 'status' => status, 'body' => { 'code' => code, 'msg' => msg } }
  BN_1021 = 'Timestamp for this request is outside of the recvWindow.'
  BN_1021_AHEAD = "Timestamp for this request was 1000ms ahead of the server's time."

  # Slice 6a: a one-asset basket selling (Bot::Composition::OrderSetter#one_asset_sell_orders_data) on Kraken, Binance and
  # Alpaca, by N base (plain or split by Smart Intervals) and for N quote, market and limit.
  def sell_scenarios
    started = '2026-09-01T10:00:00.123456Z'
    after = ->(n) { (Time.iso8601(started) + (n * 1.day) + 1.second).iso8601(6) }
    # Kraken's free amount is balance - hold_trade: 0.1 held by a resting sell, the rest free.
    kr_balance = ->(free) { { 'error' => [], 'result' => { 'ZEUR' => { 'balance' => '100000', 'hold_trade' => '0' },
                                                         'XXBT' => { 'balance' => (BigDecimal(free) + BigDecimal('0.1')).to_s('F'), 'hold_trade' => '0.1' } } } }
    bn_holding = ->(btc) { ok('makerCommission' => 10, 'canTrade' => true, 'balances' => [{ 'asset' => 'BTC', 'free' => btc, 'locked' => '0.1' },
                                                                                       { 'asset' => 'USDT', 'free' => '100000.00000000', 'locked' => '0' }]) }
    # Alpaca's free is qty_available: 0.1 sits on a resting sell (qty counts it, qty_available does not).
    al_held = ->(free) { ok([position('BTCUSD', (BigDecimal(free) + BigDecimal('0.1')).to_s('F')).merge('qty_available' => free)]) }
    al_book = ->(bid) { ok('quotes' => { 'BTC/USD' => { 'ap' => 64_321.479, 'as' => 0.5, 'bp' => bid, 'bs' => 0.4, 't' => '2026-09-01T10:00:00Z' } }) }
    al_fail = ->(status, code, message) { { 'status' => status, 'body' => { 'code' => code, 'message' => message } } }
    # A buy the flip cancelled, registered in flip_reserve_pending (Bot::Reversible#register_flip_reserves!, PR #597): the
    # tick's sweep reads its final state and returns the unspent reserve once; a cancel still pending returns nothing yet.
    al_flipped = { 'status' => 0, 'external_status' => 0, 'external_id' => 'OBUY-1', 'order_type' => 0, 'quote_amount' => '60', 'price' => '64150',
                   'amount_exec' => '0', 'quote_amount_exec' => '0', 'created_at' => '2026-09-01 10:00:01' }
    al_resting = { 'status' => 0, 'external_status' => 1, 'external_id' => 'OREST-1', 'order_type' => 1, 'side' => 1, 'amount' => '0.1', 'price' => '64500',
                   'amount_exec' => '0', 'quote_amount_exec' => '0', 'created_at' => '2026-09-01 10:00:01' }
    venues = {
      'alpaca' => {
        'ticker' => { 'base_decimals' => 9, 'quote_decimals' => 2, 'price_decimals' => 2, 'minimum_base_size' => '0.000027', 'minimum_quote_size' => '1' },
        'http' => { 'GET /v1beta3/crypto/us/latest/quotes' => [al_book.(64_300.25)], 'GET /v1beta3/crypto/us/latest/trades' => [trades(64_310.75)],
                    'POST /v2/orders' => [ok(alpaca_order('OTX-1', 'pending_new').merge('side' => 'sell'))], 'GET /v2/account' => [account('100000')],
                    'GET /v2/positions' => [al_held.('0.5')] },
        'free' => ->(btc) { { 'GET /v2/positions' => [al_held.(btc)] } },
        'balance_error' => { 'GET /v2/positions' => [al_fail.(403, 40_310_000, 'forbidden.')] },
        'no_holding' => { 'GET /v2/positions' => [ok([])] },
        'rejected' => { 'POST /v2/orders' => [al_fail.(403, 40_310_000, 'insufficient qty available for order (requested: 0.01, available: 0.004)')] },
        'no_price' => { 'GET /v1beta3/crypto/us/latest/quotes' => [al_fail.(500, 50_010_000, 'internal server error')],
                        'GET /v1beta3/crypto/us/latest/trades' => [al_fail.(500, 50_010_000, 'internal server error')] },
        'zero_price' => { 'GET /v1beta3/crypto/us/latest/quotes' => [al_book.(0)], 'GET /v1beta3/crypto/us/latest/trades' => [trades(0)] },
        'extra' => {
          'resting_sell' => { 'transactions' => [al_resting],
                              'http' => { 'GET /v2/orders/OREST-1' => [ok(alpaca_order('OREST-1', 'new', type: 'limit', notional: nil, qty: '0.1', limit_price: '64500').merge('side' => 'sell'))] } },
          'reserve_settled' => { 'transactions' => [al_flipped], 'transient' => { 'flip_reserve_pending' => [1], 'missed_quote_amount' => '0' },
                                 'http' => { 'GET /v2/orders/OBUY-1' => [ok(alpaca_order('OBUY-1', 'canceled', filled_qty: '0.0003', filled_avg_price: '64000'))] } },
          'reserve_pending_cancel' => { 'transactions' => [al_flipped], 'transient' => { 'flip_reserve_pending' => [1], 'missed_quote_amount' => '0' },
                                        'http' => { 'GET /v2/orders/OBUY-1' => [ok(alpaca_order('OBUY-1', 'pending_cancel'))] } }
        }
      },
      'kraken' => {
        'ticker' => { 'base_decimals' => 8, 'quote_decimals' => 5, 'price_decimals' => 1, 'minimum_base_size' => '0.00005', 'minimum_quote_size' => '0.5' },
        'http' => { '/0/public/Ticker' => [ticker_body('49990.1', '50000.2', '49995.3')], '/0/private/AddOrder' => [{ 'error' => [], 'result' => { 'txid' => ['OTX-1'] } }],
                    '/0/private/BalanceEx' => [kr_balance.('0.5')], '/0/private/QueryOrders' => [query_body({})],
                    '/0/private/TradesHistory' => [{ 'error' => [], 'result' => { 'trades' => {}, 'count' => 0 } }] },
        'free' => ->(xbt) { { '/0/private/BalanceEx' => [kr_balance.(xbt)] } },
        'balance_error' => { '/0/private/BalanceEx' => [{ 'error' => ['EGeneral:Internal error'] }] },
        'no_holding' => { '/0/private/BalanceEx' => [{ 'error' => [], 'result' => { 'ZEUR' => { 'balance' => '100000', 'hold_trade' => '0' } } }] },
        'rejected' => { '/0/private/AddOrder' => [{ 'error' => ['EOrder:Insufficient funds'] }] },
        'no_price' => { '/0/public/Ticker' => [{ 'error' => ['EGeneral:Internal error'] }] },
        'zero_price' => { '/0/public/Ticker' => [ticker_body('0', '0', '0')] }
      },
      'binance' => {
        'ticker' => { 'base_decimals' => 5, 'quote_decimals' => 8, 'price_decimals' => 2, 'minimum_base_size' => '0.00001', 'minimum_quote_size' => '5' },
        'http' => { 'GET /api/v3/ticker/bookTicker' => [bn_book('64000.11000000')], 'GET /api/v3/ticker/price' => [bn_last('64000.05000000')],
                    'POST /api/v3/order' => [ok('symbol' => 'BTCUSDT', 'orderId' => 28_457, 'orderListId' => -1, 'clientOrderId' => 'x', 'transactTime' => 1_788_000_000_000)],
                    'GET /api/v3/account' => [bn_holding.('0.5')], 'GET /api/v3/allOrders' => [ok([])], 'GET /api/v3/myTrades' => [ok([])] },
        'free' => ->(btc) { { 'GET /api/v3/account' => [bn_holding.(btc)] } },
        'balance_error' => { 'GET /api/v3/account' => [bn_fail(400, -2015, 'Invalid API-key, IP, or permissions for action.')] },
        'no_holding' => { 'GET /api/v3/account' => [ok('makerCommission' => 10, 'canTrade' => true, 'balances' => [{ 'asset' => 'USDT', 'free' => '1', 'locked' => '0' }])] },
        'rejected' => { 'POST /api/v3/order' => [bn_fail(400, -2010, 'Account has insufficient balance for requested action.')] },
        'no_price' => { 'GET /api/v3/ticker/bookTicker' => [bn_fail(400, -1121, 'Invalid symbol.')], 'GET /api/v3/ticker/price' => [bn_fail(400, -1121, 'Invalid symbol.')] },
        'zero_price' => { 'GET /api/v3/ticker/bookTicker' => [ok('symbol' => 'BTCUSDT', 'bidPrice' => '0.00000000', 'bidQty' => '0', 'askPrice' => '0.00000000', 'askQty' => '0')],
                          'GET /api/v3/ticker/price' => [bn_last('0.00000000')] }
      },
      # Plan VB §1.5: a market sell goes out at the crossed bid Rails' #set_market_order re-reads, its base as sized (no divergence).
      'gemini' => {
        'ticker' => { 'base_decimals' => 8, 'quote_decimals' => 2, 'price_decimals' => 2, 'minimum_base_size' => '0.00001', 'minimum_quote_size' => '0' },
        'http' => { 'GET /v1/pubticker/btcusd' => [gm_ticker('64000.01', '64000.11', '64000.05')],
                    'POST /v1/order/new' => [ok(gm_order(7_612_345_678, live: false, side: 'sell', executed: '0.01', original: '0.01', avg: '64000'))],
                    'POST /v1/balances' => [gm_balances('100000', btc: '0.5')] },
        'free' => ->(btc) { { 'POST /v1/balances' => [gm_balances('100000', btc:)] } },
        'balance_error' => { 'POST /v1/balances' => [gm_err(400, 'InvalidApiKey', 'Invalid API key')] },
        'no_holding' => { 'POST /v1/balances' => [ok([{ 'type' => 'exchange', 'currency' => 'USD', 'amount' => '1', 'available' => '1', 'availableForWithdrawal' => '1' }])] },
        'rejected' => { 'POST /v1/order/new' => [gm_err(400, 'InsufficientFunds', 'Failed to place sell order: insufficient funds')] },
        'no_price' => { 'GET /v1/pubticker/btcusd' => [gm_err(400, 'InvalidSymbol', 'Invalid symbol')] },
        'zero_price' => { 'GET /v1/pubticker/btcusd' => [gm_ticker('0', '0', '0')] }
      }
    }
    sold = { 'status' => 0, 'external_status' => 2, 'external_id' => 'SOLD-1', 'order_type' => 0, 'side' => 1, 'amount' => '0.01', 'amount_exec' => '0.01',
             'quote_amount' => '500', 'quote_amount_exec' => '500', 'price' => '50000', 'created_at' => '2026-09-01 10:00:01' }
    bought = sold.merge('external_id' => 'BOUGHT-1', 'side' => 0, 'amount' => '0.02', 'amount_exec' => '0.02', 'quote_amount' => '1000', 'quote_amount_exec' => '1000',
                        'created_at' => '2026-08-30 10:00:01')
    sell = { 'direction' => 'selling', 'sell_interval' => 'day' }
    modes = { 'base_market' => sell.merge('sell_denomination' => 'base', 'sell_amount' => 0.01),
              'base_limit' => sell.merge('sell_denomination' => 'base', 'sell_amount' => 0.01, 'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.0025),
              'base_smart' => sell.merge('sell_denomination' => 'base', 'sell_amount' => 0.03, 'smart_intervaled' => true, 'smart_interval_quote_amount' => 20.0,
                                         'smart_interval_base_amount' => 0.01),
              'quote_market' => sell.merge('sell_denomination' => 'quote', 'sell_quote_amount' => 100.0),
              'quote_limit' => sell.merge('sell_denomination' => 'quote', 'sell_quote_amount' => 100.0, 'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.0025) }
    venues.flat_map do |venue, vn|
      variants = {
        'first_tick' => { 'at' => (Time.iso8601(started) + 0.5).iso8601(6) },
        'on_schedule' => { 'at' => after.(1), 'transactions' => [bought, sold] },
        'free_below' => { 'at' => after.(1), 'http' => vn['free'].('0.004') },
        'free_zero' => { 'at' => after.(1), 'http' => vn['free'].('0') },
        'no_holding' => { 'at' => after.(1), 'http' => vn['no_holding'] },
        'below_floor' => { 'at' => after.(1), 'http' => vn['free'].('0.000001') },
        'empty_sentence' => { 'at' => after.(1), 'settings' => { 'sell_amount' => nil, 'sell_quote_amount' => nil } },
        'carry_frozen' => { 'at' => after.(1), 'transient' => { 'missed_quote_amount' => '37.5' } },
        'rejected_insufficient' => { 'at' => after.(1), 'http' => vn['rejected'] },
        'no_price' => { 'at' => after.(1), 'http' => vn['no_price'] },
        'zero_price' => { 'at' => after.(1), 'http' => vn['zero_price'] },
        'balance_error' => { 'at' => after.(1), 'http' => vn['balance_error'] }
      }.merge((vn['extra'] || {}).transform_values { |v| v.merge('at' => after.(1)) })
      modes.flat_map do |mode, settings|
        variants.map do |name, v|
          { 'name' => "sell-#{venue}-#{mode}-#{name}", 'venue' => (venue unless venue == 'kraken'), 'interval' => 'week', 'quote_amount' => 60.0,
            'started_at' => started, 'settings' => settings.merge(v.fetch('settings', {})), 'transient' => v.fetch('transient', {}),
            'transactions' => v.fetch('transactions', []), 'ticker' => vn['ticker'], 'at' => v.fetch('at'),
            'script' => { (venue == 'kraken' ? 'http' : venue) => vn['http'].merge(v.fetch('http', {})) }, 'tick' => true }
        end
      end
    end
  end

  # Slice 6b: a wider Alpaca basket selling (Bot::Composition::OrderSetter#get_sell_orders_data). Members held at the ask,
  # valued at the last trade; free is each position's qty_available.
  def basket_sell_scenarios
    modes = { 'market' => {}, 'limit' => { 'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.0025 } }
    denominations = { 'quote' => { 'sell_denomination' => 'quote', 'sell_quote_amount' => 100.0 },
                      'base' => { 'sell_denomination' => 'base', 'sell_amount' => 0.01 } }
    held = lambda do |n, sym, qty, **extra|
      ask = BASKET_PRICES.fetch(sym)[0].to_d
      { 'asset' => sym, 'status' => 0, 'external_status' => 2, 'external_id' => "OHELD-#{n}", 'order_type' => 0, 'amount' => qty, 'amount_exec' => qty,
        'quote_amount' => (qty.to_d * ask).to_s('F'), 'quote_amount_exec' => (qty.to_d * ask).to_s('F'), 'price' => ask.to_s('F'),
        'created_at' => '2026-09-01 10:00:01' }.merge(extra)
    end
    pos = ->(sym, qty, free = qty) { position("#{sym}USD", qty).merge('qty_available' => free) }
    sold = ->(n, sym) { ok(alpaca_order("OTX-#{n}", 'pending_new', symbol: "#{sym}/USD", notional: nil).merge('side' => 'sell')) }
    holdings = { 'w50' => { 'BTC' => '0.01', 'ETH' => '0.02' }, 'tiers' => { 'BTC' => '0.01', 'ETH' => '0.02', 'SOL' => '0.1' } }
    list = []
    WEIGHTS.slice('w50', 'tiers').each do |w, weights|
      qty = holdings.fetch(w)
      rows = qty.each_with_index.map { |(sym, q), i| held.(i + 1, sym, q) }
      positions = ->(over = {}) { ok(qty.map { |sym, q| pos.(sym, q, over.fetch(sym, q)) }) }
      resting = { 'asset' => 'BTC', 'status' => 0, 'external_status' => 1, 'external_id' => 'OREST-1', 'order_type' => 1, 'side' => 1, 'amount' => '0.009',
                  'price' => '65000', 'amount_exec' => '0', 'quote_amount_exec' => '0', 'created_at' => '2026-09-01 10:00:02' }
      rest_reply = ok(alpaca_order('OREST-1', 'new', type: 'limit', notional: nil, qty: '0.009', limit_price: '65000').merge('side' => 'sell'))
      no_eth = basket_trades.tap { |r| r['body'] = { 'trades' => r['body']['trades'].except('ETH/USD') } }
      states = {
        'over_top' => {},
        # Every member at its own value and target: the tie goes to composition order.
        # 64 USD of every member at these last trades (w50: one value and one target each, so composition order decides).
        'tie' => { 'rows' => qty.keys.each_with_index.map { |sym, i| held.(i + 1, sym, { 'BTC' => '0.001', 'ETH' => '0.0256', 'SOL' => '0.4' }.fetch(sym)) },
                   'positions' => ok(qty.keys.map { |sym| pos.(sym, '10') }),
                   'http' => { 'GET /v1beta3/crypto/us/latest/trades' => [basket_trades('BTC' => [64_000.0, 64_000.0], 'ETH' => [2500.0, 2500.0], 'SOL' => [150.0, 160.0])] } },
        'resting_top' => { 'rows' => rows + [resting], 'http' => { 'GET /v2/orders/OREST-1' => [rest_reply] } },
        'free_below' => { 'positions' => positions.('BTC' => '0.001') },
        'floor_handover' => { 'positions' => positions.('BTC' => '0.00001') },
        'nothing_free_top' => { 'positions' => positions.('BTC' => '0') },
        'all_under_floor' => { 'settings' => { 'sell_quote_amount' => 0.5 } },
        # 1 BTC a merge brought from Kraken: the 1,000 USD withdrawal takes only this venue's 0.01 of it, then moves on.
        'foreign_crypto' => { 'rows' => rows + [held.(9, 'BTC', '1.0', 'foreign' => true, 'external_id' => 'OFOREIGN-1')], 'positions' => positions.('BTC' => '1.01'),
                              'settings' => { 'sell_quote_amount' => 1000.0 } },
        'unpriced_member' => { 'http' => { 'GET /v1beta3/crypto/us/latest/trades' => [no_eth] } },
        'stale_feed' => { 'http' => { 'GET /v1beta3/crypto/us/latest/trades' => [FIVE_XX] } },
        'feed_unreachable' => { 'http' => { 'GET /v1beta3/crypto/us/latest/trades' => [DATA_PRE_SEND] } },
        'no_holdings' => { 'rows' => [] },
        'rejected' => { 'http' => { 'POST /v2/orders' => [{ 'status' => 403, 'body' => { 'code' => 40_310_000, 'message' => 'insufficient balance for BTC (requested: 0.0015, available: 0)' } }] } },
        'balance_error' => { 'positions' => { 'status' => 403, 'body' => { 'code' => 40_310_000, 'message' => 'forbidden.' } } }
      }
      states.each do |state, st|
        modes.each do |mode, msettings|
          denominations.each do |denom, dsettings|
            next if denom == 'base' && !%w[over_top no_holdings stale_feed].include?(state) # a zero withdrawal: valued, then nothing
            settings = { 'direction' => 'selling' }.merge(dsettings, msettings, st.fetch('settings', {}))
            http = { 'GET /v2/positions' => [st.fetch('positions', positions.())], 'POST /v2/orders' => [sold.(1, 'BTC'), sold.(2, 'ETH'), sold.(3, 'SOL')] }
                   .merge(st.fetch('http', {}))
            list << basket("basket-sell-#{w}-#{state}-#{denom}-#{mode}", weights, at: basket_at(1), settings:, transactions: st.fetch('rows', rows), http:)
          end
        end
      end
    end
    list.concat(stock_basket_sell_scenarios(modes), cb_basket_sell_scenarios(modes, denominations))
  end

  # A stock basket selling: the market open (it sells), closed (it parks), and a merged holding (a share may have split:
  # nothing of it is this venue's to sell).
  def stock_basket_sell_scenarios(modes)
    at = (Time.iso8601(STOCK_STARTED).utc.beginning_of_day + 1.day + 14.hours + 1).iso8601(6)
    bought = lambda do |n, sym, qty, price, **extra|
      { 'symbol' => sym, 'status' => 0, 'external_status' => 2, 'external_id' => "OHELD-#{n}", 'order_type' => 0, 'amount' => qty, 'amount_exec' => qty,
        'quote_amount' => (qty.to_d * price.to_d).to_s('F'), 'quote_amount_exec' => (qty.to_d * price.to_d).to_s('F'), 'price' => price,
        'created_at' => '2026-09-01 14:00:01' }.merge(extra)
    end
    rows = [bought.(1, 'AAPL', '2', '187.5'), bought.(2, 'MSFT', '0.1', '400')]
    stock_pos = ->(sym, qty) { { 'symbol' => sym, 'asset_class' => 'us_equity', 'qty' => qty, 'qty_available' => qty } }
    snapshots = ok('AAPL' => { 'latestTrade' => { 'p' => 187.41 } }, 'MSFT' => { 'latestTrade' => { 'p' => 401.2 } })
    sold = ->(id, sym) { ok(stock_order(id, 'new', sym:, notional: nil).merge('side' => 'sell')) }
    states = {
      'open' => {},
      'closed' => { 'http' => { 'GET /v2/clock' => [closed_clock(at)] } },
      'foreign' => { 'rows' => rows + [bought.(9, 'AAPL', '5', '150', 'foreign' => true, 'external_id' => 'OFOREIGN-1')], 'aapl' => '7' }
    }
    states.flat_map do |state, st|
      modes.map do |mode, msettings|
        http = { 'GET /v2/clock' => [open_clock(at)], 'GET /v2/stocks/snapshots' => [snapshots],
                 'GET /v2/stocks/AAPL/quotes/latest' => [stock_quote('AAPL', 187.43)], 'GET /v2/stocks/AAPL/trades/latest' => [stock_trade('AAPL', 187.41)],
                 'GET /v2/stocks/MSFT/quotes/latest' => [stock_quote('MSFT', 401.25)], 'GET /v2/stocks/MSFT/trades/latest' => [stock_trade('MSFT', 401.2)],
                 'POST /v2/orders' => [sold.('OTX-1', 'AAPL'), sold.('OTX-2', 'MSFT')], 'GET /v2/account' => [stock_account('100000', '200000')],
                 'GET /v2/positions' => [ok([stock_pos.('AAPL', st.fetch('aapl', '2')), stock_pos.('MSFT', '0.1')])] }.merge(st.fetch('http', {}))
        { 'name' => "basket-sell-stock-#{state}-#{mode}", 'venue' => 'alpaca', 'asset' => 'stock', 'interval' => 'day', 'quote_amount' => 60.0,
          'started_at' => STOCK_STARTED, 'settings' => { 'direction' => 'selling', 'sell_denomination' => 'quote', 'sell_quote_amount' => 100.0 }.merge(msettings),
          'transient' => {}, 'transactions' => st.fetch('rows', rows), 'members' => [['AAPL', 0.5], ['MSFT', 0.5]], 'at' => at,
          'script' => { 'alpaca' => http }, 'tick' => true }
      end
    end
  end

  # ---- Plan CB: wider baskets on Kraken (EUR), Binance (USDT) and KuCoin (USDT), valued by each venue's bulk price read
  # (Exchange#get_tickers_prices), sold at its bid, a member's free amount read from the venue's own balances.
  CB_VENUES = %w[kraken binance kucoin].freeze
  CB_SCRIPT = { 'kraken' => 'kraken_http', 'binance' => 'binance', 'kucoin' => 'kucoin' }.freeze
  # Kraken's own codes in BalanceEx (the gem maps XXBT to XBT, XETH to ETH and ZEUR to EUR) and its bulk Ticker's key for BTC.
  CB_BALANCE_CODE = { 'kraken' => { 'BTC' => 'XXBT', 'ETH' => 'XETH', 'SOL' => 'SOL', 'EUR' => 'ZEUR' } }.freeze
  KRAKEN_NOT_SENT = { 'network' => 'kraken_pre_send', 'message' => 'Faraday::ConnectionFailed: Connection refused - connect(2) for "api.kraken.com" port 443' }.freeze
  CB_NOT_SENT = { 'kraken' => KRAKEN_NOT_SENT }.merge(VENUES.transform_values(&:not_sent)).freeze
  CB_QUOTE = { 'kraken' => 'EUR', 'binance' => 'USDT', 'kucoin' => 'USDT' }.freeze

  def cb_pair(venue, sym) = VENUE_TICKER.fetch(venue).fetch(sym)
  def cb_last(venue, sym) = VENUE_PRICES.fetch(venue).fetch(sym)[2]

  # The bulk price read, every member at its last price unless `last` says otherwise (nil: the venue does not price it;
  # a Hash: that row as given). Kraken keys BTC as XXBTZEUR, so its XBTEUR is asked again by pair.
  def cb_bulk(venue, last = {})
    price = ->(sym) { last.key?(sym) ? last[sym] : cb_last(venue, sym) }
    priced = %w[BTC ETH SOL].reject { |sym| last.key?(sym) && last[sym].nil? }
    case venue
    when 'kraken'
      rows = priced.to_h do |sym|
        p = price.(sym)
        [sym == 'BTC' ? 'XXBTZEUR' : cb_pair(venue, sym), p.is_a?(Hash) ? p : kr_ticker('x', p, p, p)['result']['x']]
      end
      { 'GET /0/public/Ticker' => [ok('error' => [], 'result' => rows.merge('ADAEUR' => kr_ticker('x', '0.4', '0.4', '0.4')['result']['x']))] }
    when 'binance'
      rows = priced.map { |sym| price.(sym).is_a?(Hash) ? price.(sym) : { 'symbol' => cb_pair(venue, sym), 'price' => price.(sym) } }
      { 'GET /api/v3/ticker/price' => [ok(rows + [{ 'symbol' => 'ADAUSDT', 'price' => '0.40000000' }])] }
    else
      rows = priced.map { |sym| price.(sym).is_a?(Hash) ? price.(sym) : { 'symbol' => cb_pair(venue, sym), 'last' => price.(sym) } }
      { 'GET /api/v1/market/allTickers' => [kc_ok('time' => 1_788_000_000_000, 'ticker' => rows + [{ 'symbol' => 'ADA-USDT', 'last' => '0.4' }])] }
    end
  end

  # Each member's book (its bid prices a sell, its ask a buy) and last price, by pair. On Kraken the same reply answers
  # the bulk read's fallback for XBTEUR; `unknown` answers a member as a pair the venue does not know.
  def cb_books(venue, unknown: [])
    VENUE_PRICES.fetch(venue).flat_map do |sym, (bid, ask, last)|
      pair = cb_pair(venue, sym)
      gone = unknown.include?(sym)
      case venue
      when 'kraken' then [["GET /0/public/Ticker?pair=#{pair}", [ok(gone ? { 'error' => ['EQuery:Unknown asset pair'] } : kr_ticker(pair, bid, ask, last))]]]
      when 'binance'
        bad = bn_fail(400, -1121, 'Invalid symbol.')
        [["GET /api/v3/ticker/bookTicker?symbol=#{pair}", [gone ? bad : ok('symbol' => pair, 'bidPrice' => bid, 'bidQty' => '1.5', 'askPrice' => ask, 'askQty' => '0.2')]],
         ["GET /api/v3/ticker/price?symbol=#{pair}", [gone ? bad : ok('symbol' => pair, 'price' => last)]]]
      else [["GET /api/v1/market/orderbook/level2_20?symbol=#{pair}", [gone ? ok('code' => '400100', 'msg' => 'symbol not exists') : kc_book(ask, bid:)]]]
      end
    end.to_h
  end

  # The venue's balances: each member's free amount, and the quote's.
  def cb_balances(venue, free, quote: '100000')
    all = free.merge(CB_QUOTE.fetch(venue) => quote)
    case venue
    when 'kraken'
      code = CB_BALANCE_CODE.fetch('kraken')
      { 'POST /0/private/BalanceEx' => [ok('error' => [], 'result' => all.to_h { |sym, q| [code.fetch(sym, sym), { 'balance' => q, 'hold_trade' => '0' }] })] }
    when 'binance'
      { 'GET /api/v3/account' => [ok('makerCommission' => 10, 'canTrade' => true, 'balances' => all.map { |sym, q| { 'asset' => sym, 'free' => q, 'locked' => '0' } })] }
    else
      { 'GET /api/v1/accounts' => [kc_ok(all.map { |sym, q| { 'id' => sym.downcase, 'currency' => sym, 'type' => 'trade', 'balance' => q, 'available' => q, 'holds' => '0' } })] }
    end
  end

  # The n-th order the venue accepts, by its own id.
  def cb_placed(venue, n)
    case venue
    when 'kraken' then ok('error' => [], 'result' => { 'txid' => ["OTX-#{n}"] })
    when 'binance' then ok('symbol' => 'BTCUSDT', 'orderId' => 28_456 + n, 'orderListId' => -1, 'clientOrderId' => 'x', 'transactTime' => 1_788_000_000_000)
    else kc_ok('orderId' => "6716f0a2c1b6d10007a3b1c#{n}")
    end
  end

  def cb_place_key(venue) = venue == 'kraken' ? 'POST /0/private/AddOrder' : VENUES.fetch(venue).scripted.order

  # A refusal of the placement as each venue words one.
  def cb_rejected(venue)
    case venue
    when 'kraken' then ok('error' => ['EOrder:Insufficient funds'])
    when 'binance' then bn_fail(400, -2010, 'Account has insufficient balance for requested action.')
    else kc_err(400, '200004', 'Balance insufficient!')
    end
  end

  # What a tick reads beside prices, balances and placements: no open orders, no trades.
  def cb_quiet(venue)
    case venue
    when 'kraken'
      { 'POST /0/private/QueryOrders' => [ok(query_body({}))], 'POST /0/private/TradesHistory' => [ok('error' => [], 'result' => { 'trades' => {}, 'count' => 0 })] }
    when 'binance' then { 'GET /api/v3/allOrders' => [ok([])], 'GET /api/v3/myTrades' => [ok([])] }
    else { 'GET /api/v1/fills' => [kc_fills([])] }
    end
  end

  def cb_http(venue, free, last: {}, unknown: [], quote: '100000')
    { cb_place_key(venue) => (1..6).map { |n| cb_placed(venue, n) } }.merge(cb_quiet(venue), cb_books(venue, unknown:), cb_balances(venue, free, quote:),
                                                                         cb_bulk(venue, last))
  end

  # A closed buy that left `qty` of the member, at its ask.
  def cb_held(venue, n, sym, qty, **extra)
    ask = VENUE_PRICES.fetch(venue).fetch(sym)[1].to_d
    { 'asset' => sym, 'status' => 0, 'external_status' => 2, 'external_id' => "OHELD-#{n}", 'order_type' => 0, 'amount' => qty, 'amount_exec' => qty,
      'quote_amount' => (qty.to_d * ask).to_s('F'), 'quote_amount_exec' => (qty.to_d * ask).to_s('F'), 'price' => ask.to_s('F'),
      'created_at' => '2026-09-01 10:00:01' }.merge(extra)
  end

  # The venue's id for a resting sell row and its open answer to the tick's sweep.
  def cb_resting(venue, sym, qty, price)
    pair = cb_pair(venue, sym)
    case venue
    when 'kraken'
      ['OREST-1', { 'POST /0/private/QueryOrders' => [ok(query_body('OREST-1' => { 'status' => 'open', 'vol' => qty, 'vol_exec' => '0', 'cost' => '0', 'price' => '0',
                                                                                   'oflags' => '', 'descr' => { 'pair' => pair, 'type' => 'sell', 'ordertype' => 'limit', 'price' => price } }))] }]
    when 'binance'
      ["#{pair}-30001", { 'GET /api/v3/allOrders' => [ok([bn_order(30_001, 'NEW', type: 'LIMIT', price:, qty:).merge('symbol' => pair, 'side' => 'SELL')])] }]
    else
      id = '6716f0a2c1b6d10007a3b0e1'
      [id, { "GET /api/v1/orders/#{id}" => [kc_ok(kc_order(id, active: true, type: 'limit', price:, size: qty, funds: '0').merge('symbol' => pair, 'side' => 'sell'))] }]
    end
  end

  # T3: a wider basket's ranked DCA-out (get_sell_orders_data) on each crypto venue.
  def cb_basket_sell_scenarios(modes, denominations)
    holdings = { 'w50' => { 'BTC' => '0.01', 'ETH' => '0.02' }, 'tiers' => { 'BTC' => '0.01', 'ETH' => '0.02', 'SOL' => '0.1' } }
    unreadable = { 'kraken' => { 'ETH' => kr_ticker('x', 'x', 'x', 'x')['result']['x'] }, 'binance' => { 'ETH' => 'abc' }, 'kucoin' => { 'ETH' => '12x' } }
    raised = { 'kraken' => { 'ETH' => { 'a' => %w[1 1 1] } }, 'binance' => { 'ETH' => { 'symbol' => 'ETHUSDT' } }, 'kucoin' => { 'ETH' => { 'last' => '2500.47' } } }
    stale = { 'kraken' => ok('error' => ['EService:Unavailable']), 'binance' => bn_fail(503, -1001, 'Internal error; unable to process your request. Please try again.'),
              'kucoin' => ok('code' => '400100', 'msg' => 'Too many requests') }
    bulk_key = { 'kraken' => 'GET /0/public/Ticker', 'binance' => 'GET /api/v3/ticker/price', 'kucoin' => 'GET /api/v1/market/allTickers' }
    CB_VENUES.flat_map do |venue|
      floor = { 'kraken' => 0.5, 'binance' => 0.5, 'kucoin' => 0.05 }.fetch(venue)
      WEIGHTS.slice('w50', 'tiers').flat_map do |w, weights|
        qty = holdings.fetch(w)
        rows = qty.each_with_index.map { |(sym, q), i| cb_held(venue, i + 1, sym, q) }
        rest_id, rest_http = cb_resting(venue, 'BTC', '0.009', '65000')
        resting = { 'asset' => 'BTC', 'status' => 0, 'external_status' => 1, 'external_id' => rest_id, 'order_type' => 1, 'side' => 1, 'amount' => '0.009',
                    'price' => '65000', 'amount_exec' => '0', 'quote_amount_exec' => '0', 'created_at' => '2026-09-01 10:00:02' }
        # The prices of a tie: every member worth 100 of the quote.
        tie_last = { 'BTC' => '50000', 'ETH' => '2500', 'SOL' => '125' }
        tie_qty = { 'BTC' => '0.002', 'ETH' => '0.04', 'SOL' => '0.8' }
        states = {
          'over_top' => {},
          # (Kraken prices XBTEUR by its pair, the bulk read naming it XXBTZEUR.)
          'tie' => { 'rows' => qty.keys.each_with_index.map { |sym, i| cb_held(venue, i + 1, sym, tie_qty.fetch(sym)) }, 'free' => qty.keys.to_h { |sym| [sym, '10'] },
                     'last' => tie_last.slice(*qty.keys),
                     'http' => venue == 'kraken' ? { 'GET /0/public/Ticker?pair=XBTEUR' => [ok(kr_ticker('XBTEUR', '49990.1', '50000.2', '50000'))] } : {} },
          'resting_top' => { 'rows' => rows + [resting], 'http' => rest_http },
          'free_below' => { 'free' => qty.merge('BTC' => '0.001') },
          'floor_handover' => { 'free' => qty.merge('BTC' => '0.000001') },
          'all_under_floor' => { 'settings' => { 'sell_quote_amount' => floor } },
          'unpriced_member' => venue == 'kucoin' ? { 'last' => { 'ETH' => nil } } : { 'last' => { 'ETH' => nil }, 'unknown' => ['ETH'] },
          'unreadable_price' => { 'last' => unreadable.fetch(venue) },
          'raised_price' => { 'last' => raised.fetch(venue) },
          'stale_feed' => { 'http' => { bulk_key.fetch(venue) => [stale.fetch(venue)] } },
          'feed_unreachable' => { 'http' => { bulk_key.fetch(venue) => [CB_NOT_SENT.fetch(venue)] } },
          'no_holdings' => { 'rows' => [] },
          'rejected' => { 'http' => { cb_place_key(venue) => [cb_rejected(venue)] } },
          # A sell cancelled after selling part of BTC: the rest is still held, and the ranking reads the ledger as it is now.
          'cancelled_partial_sell' => { 'rows' => rows + [cb_held(venue, 8, 'BTC', '0.004', 'side' => 1, 'external_status' => 3, 'external_id' => 'OCAN-1',
                                                                 'amount' => '0.008', 'quote_amount' => nil)] }
        }
        states.flat_map do |state, st|
          modes.flat_map do |mode, msettings|
            denominations.filter_map do |denom, dsettings|
              next if denom == 'base' && !%w[over_top no_holdings stale_feed].include?(state)

              settings = { 'direction' => 'selling' }.merge(dsettings, msettings, st.fetch('settings', {}))
              http = cb_http(venue, st.fetch('free', qty), last: st.fetch('last', {}), unknown: st.fetch('unknown', [])).merge(st.fetch('http', {}))
              { 'name' => "basket-sell-#{venue}-#{w}-#{state}-#{denom}-#{mode}", 'venue' => venue, 'interval' => 'day', 'quote_amount' => 60.0,
                'started_at' => BASKET_STARTED, 'settings' => settings, 'settings_changed_at' => nil, 'transient' => {}, 'transactions' => st.fetch('rows', rows),
                'ticker' => VENUE_PAIRS.fetch(venue)['BTC'], 'members' => weights, 'at' => basket_at(1), 'script' => { CB_SCRIPT.fetch(venue) => http }, 'tick' => true }
            end
          end
        end
      end
    end
  end

  # Slice 6b: threshold rebalancing (Bot::Rebalancer through Bot::RebalanceJob) on a stopped Alpaca basket, one fire per
  # entry of `rebalance_at` (4 hours apart). BTC holds 0.01 (640 USD) beside 0.04 ETH (100 USD): far over a 5-point band.
  def rebalance_scenarios
    held = lambda do |n, sym, qty, **extra|
      ask = BASKET_PRICES.fetch(sym)[0].to_d
      { 'asset' => sym, 'status' => 0, 'external_status' => 2, 'external_id' => "OHELD-#{n}", 'order_type' => 0, 'amount' => qty, 'amount_exec' => qty,
        'quote_amount' => (qty.to_d * ask).to_s('F'), 'quote_amount_exec' => (qty.to_d * ask).to_s('F'), 'price' => ask.to_s('F'),
        'created_at' => '2026-09-01 10:00:01' }.merge(extra)
    end
    pos = ->(sym, qty) { position("#{sym}USD", qty) }
    fires = (1..3).map { |k| (Time.iso8601(BASKET_STARTED).beginning_of_day + 12.hours + 30.minutes + (4 * (k - 1)).hours).iso8601(6) }
    sell = ->(status, filled, avg) { ok(alpaca_order('OTX-1', status, symbol: 'BTC/USD', notional: nil, qty: '0.004218784', filled_qty: filled, filled_avg_price: avg).merge('side' => 'sell')) }
    buy = ->(id, status, notional, filled, avg, sym = 'ETH') { ok(alpaca_order(id, status, symbol: "#{sym}/USD", notional:, filled_qty: filled, filled_avg_price: avg)) }
    filled_sell = sell.('filled', '0.004218784', '63990')
    holdings = { 'w50' => { 'BTC' => '0.01', 'ETH' => '0.04' }, 'tiers' => { 'BTC' => '0.01', 'ETH' => '0.04', 'SOL' => '0.2' } }
    # The swap's buy: 270 USD of the most underweight member after BTC's sale (ETH at 50/50, SOL in the tiers), covering
    # the 269.96 the sell realized, so the swap completes.
    whole_buy = { 'w50' => ['0.108', '2500', 'ETH'], 'tiers' => ['1.8', '150', 'SOL'] }
    owed = { 'rebalance_pending' => { 'phase' => 'buying', 'sell_transaction_id' => nil, 'buy_transaction_id' => nil,
                                      'remaining_quote_amount' => '120.0', 'buy_attempted' => false } }
    list = []
    WEIGHTS.slice('w50', 'tiers').each do |w, weights|
      qty = holdings.fetch(w)
      rows = qty.each_with_index.map { |(sym, q), i| held.(i + 1, sym, q) }
      positions = ok(qty.map { |sym, q| pos.(sym, q) })
      resting_buy = { 'asset' => 'BTC', 'status' => 0, 'external_status' => 1, 'external_id' => 'OREST-1', 'order_type' => 1, 'amount' => '0.001',
                      'price' => '60000', 'amount_exec' => '0', 'quote_amount_exec' => '0', 'created_at' => '2026-09-01 10:00:02' }
      rest = ->(status) { { 'GET /v2/orders/OREST-1' => [ok(alpaca_order('OREST-1', status, type: 'limit', notional: nil, qty: '0.001', limit_price: '60000',
                                                                           filled_qty: status == 'filled' ? '0.001' : '0', filled_avg_price: status == 'filled' ? '60000' : nil))] } }
      states = {
        # The whole swap: the sell, its fill and the buy, the buy's fill.
        'swap' => { 'fires' => 3, 'http' => { 'GET /v2/orders/OTX-1' => [filled_sell], 'GET /v2/orders/OTX-2' => [buy.('OTX-2', 'filled', nil, *whole_buy.fetch(w))] } },
        'under_threshold' => { 'fires' => 1, 'settings' => { 'rebalance_threshold' => 0.5 } },
        'rule_off' => { 'fires' => 1, 'settings' => { 'rebalance_enabled' => false } },
        'selling' => { 'fires' => 1, 'settings' => { 'direction' => 'selling', 'sell_denomination' => 'quote', 'sell_quote_amount' => 10.0 } },
        'resting_sell' => { 'fires' => 2, 'http' => { 'GET /v2/orders/OTX-1' => [sell.('new', '0', nil)] } },
        'cancelled_partial_sell' => { 'fires' => 2, 'http' => { 'GET /v2/orders/OTX-1' => [sell.('canceled', '0.002', '63990')] } },
        'nothing_sold' => { 'fires' => 2, 'http' => { 'GET /v2/orders/OTX-1' => [sell.('canceled', '0', nil)] } },
        'partial_buy' => { 'fires' => 3, 'http' => { 'GET /v2/orders/OTX-1' => [filled_sell], 'GET /v2/orders/OTX-2' => [buy.('OTX-2', 'canceled', nil, '0.04', '2500')] } },
        'open_buy' => { 'fires' => 1, 'rows' => rows + [resting_buy], 'http' => rest.('new') },
        'open_buy_stale' => { 'fires' => 1, 'rows' => rows + [resting_buy], 'http' => rest.('filled') },
        'sell_below_minimum' => { 'fires' => 1, 'tickers_after' => { 'BTC' => { 'minimum_quote_size' => '500' } } },
        'buy_below_minimum' => { 'fires' => 2, 'http' => { 'GET /v2/orders/OTX-1' => [filled_sell], 'GET /v2/account' => [account('0.5')] } },
        'buy_capped' => { 'fires' => 2, 'http' => { 'GET /v2/orders/OTX-1' => [filled_sell], 'GET /v2/account' => [account('150')] } },
        'rejected' => { 'fires' => 1, 'http' => { 'POST /v2/orders' => [{ 'status' => 403, 'body' => { 'code' => 40_310_000, 'message' => 'insufficient balance for BTC' } }] } },
        'buy_rejected' => { 'fires' => 2, 'http' => { 'GET /v2/orders/OTX-1' => [filled_sell],
                                                      'POST /v2/orders' => [placed(1, 'BTC'), { 'status' => 403, 'body' => { 'code' => 40_310_000, 'message' => 'insufficient balance' } }] } },
        'pre_send' => { 'fires' => 1, 'http' => { 'POST /v2/orders' => [{ 'network' => 'pre_send', 'message' => 'Faraday::ConnectionFailed: Connection refused - connect(2) for "paper-api.alpaca.markets" port 443' }] } },
        # Rails halts at once on an unknown outcome; the engine keeps the intent and asks the venue (named divergences).
        'post_send' => { 'fires' => 1, 'http' => { 'POST /v2/orders' => [{ 'network' => 'post_send', 'message' => 'Faraday::TimeoutError: Net::ReadTimeout' }] } },
        'server_error' => { 'fires' => 1, 'http' => { 'POST /v2/orders' => [FIVE_XX] } },
        'no_order_id' => { 'fires' => 1, 'http' => { 'POST /v2/orders' => [ok('status' => 'pending_new')] } },
        # A swap already owed (rule switched off meanwhile): the buy still goes.
        'owed_buy' => { 'fires' => 1, 'settings' => { 'rebalance_enabled' => false }, 'transient' => owed },
        # Only BTC held: every drift is exactly 0.5 whatever the prices, and a threshold of 0.5 is not exceeded (strictly greater).
        'at_threshold' => { 'fires' => 1, 'settings' => { 'rebalance_threshold' => 0.5 }, 'rows' => rows.first(1),
                            'http' => { 'GET /v2/positions' => [ok([pos.('BTC', qty.fetch('BTC'))])] } },
        # The same at 0.49: the band is exceeded, and BTC sells.
        'below_drift' => { 'fires' => 1, 'settings' => { 'rebalance_threshold' => 0.49 }, 'rows' => rows.first(1),
                           'http' => { 'GET /v2/positions' => [ok([pos.('BTC', qty.fetch('BTC'))])] } },
        # Sliders summing to 120% (saved, never started): DcaMultiAsset#rebalance_targets is nil, so nothing is due...
        'unbalanced' => { 'fires' => 1, 'scale' => 1.2 },
        # ...unless a swap is already owed its buy: that one still lands.
        'unbalanced_owed_buy' => { 'fires' => 1, 'scale' => 1.2, 'transient' => owed },
        # A plain 400: not a pre-trade rejection Rails can prove, so the swap halts.
        'rejected_400' => { 'fires' => 1, 'http' => { 'POST /v2/orders' => [{ 'status' => 400, 'body' => { 'code' => 40_010_001, 'message' => 'qty must be > 0' } }] } }
      }
      states.each do |state, st|
        http = { 'GET /v2/positions' => [positions], 'POST /v2/orders' => [placed(1, 'BTC'), placed(2, 'ETH'), placed(3, 'ETH')] }.merge(st.fetch('http', {}))
        members = st['scale'] ? weights.transform_values { |v| v * st['scale'] } : weights
        list << basket("rebalance-#{w}-#{state}", members, at: fires.first, settings: { 'rebalance_enabled' => true }.merge(st.fetch('settings', {})),
                       transient: st.fetch('transient', {}), transactions: st.fetch('rows', rows), http:, tick: false,
                       rebalance_at: fires.first(st.fetch('fires')), tickers_after: st['tickers_after'], bot_columns: { 'status' => Bot.statuses[:stopped] })
      end
    end
    list << index_rebalance_scenario
    list.concat(cb_rebalance_scenarios, cb_index_rebalance_scenarios)
  end

  # Plan CB T4: a stopped Layer 1 index bot on each crypto venue whose third member left the index (in_index false, still
  # held): the exited holding is outside the arithmetic, and the swap is valued by the venue's bulk price read (Kraken's
  # names BTC XXBTZEUR, so XBTEUR is asked again by pair).
  def cb_index_rebalance_scenarios
    fire = (Time.iso8601(VI_STARTED) + 1.day + 2.hours + 30.minutes).iso8601(6)
    CB_VENUES.map do |venue|
      ranked = vi_ranked(venue, VI_L1)
      steady = vi_steady(ranked)
      held = ranked.first(3).zip([300.0, 200.0, 100.0]).map { |sym, v| vi_held(venue, sym, v) }
      last = VI_PRICES.fetch(VI_QUOTE.fetch(venue)).to_h { |sym, (_ask, l)| [sym, l] }
      bulk = case venue
             when 'kraken'
               { 'GET /0/public/Ticker' => [ok('error' => [], 'result' => last.to_h do |sym, l|
                 [sym == 'BTC' ? 'XXBTZEUR' : vi_ticker(venue, sym), vi_kraken_ticker('x', l, l)['result']['x']]
               end)] }
             else {}
             end
      # The third ranked coin left Layer 1 for the fourth, which the refresh brings in unheld.
      swapped = VI_L1.dup.tap { |l| l[2], l[3] = l[3], l[2] }
      free = held.to_h { |row| [row['symbol'], row['amount']] }
      venue_index(venue, 'category', 'rebalance_exited', at: fire, members: steady[0..1] + [[steady[2][0], steady[2][1], false]], transactions: held,
                  l1: swapped, settings: { 'rebalance_enabled' => true }, status: 'stopped', tick: false, rebalance_at: [fire],
                  over: bulk.merge(cb_balances(venue, free)))
        .merge('name' => "rebalance-#{venue}-index-exited_member")
    end
  end

  # ---- Plan CB T4: threshold rebalancing on a stopped Kraken, Binance or KuCoin basket, fire by fire. ----

  # The venue's id for its n-th accepted order (cb_placed), as Rails stores it.
  def cb_ext(venue, n, sym)
    case venue
    when 'kraken' then "OTX-#{n}"
    when 'binance' then "#{cb_pair(venue, sym)}-#{28_456 + n}"
    else "6716f0a2c1b6d10007a3b1c#{n}"
    end
  end

  # One order as the venue answers a read of it: `status` open, closed or cancelled; `filled` base and `quote` executed.
  def cb_order(venue, n, sym, side, status, qty:, filled:, quote:, price:, fee: '0')
    pair = cb_pair(venue, sym)
    buy = side == 'buy'
    case venue
    when 'kraken'
      { 'status' => { 'cancelled' => 'canceled' }.fetch(status, status), 'vol' => buy ? quote : qty, 'vol_exec' => filled, 'cost' => quote, 'fee' => fee,
        'price' => price, 'oflags' => buy ? 'viqc' : '', 'descr' => { 'pair' => pair, 'type' => side, 'ordertype' => 'market', 'price' => '0' } }
    when 'binance'
      bn_order(28_456 + n, { 'open' => 'NEW', 'closed' => 'FILLED', 'cancelled' => 'CANCELED' }.fetch(status), executed: filled, cumulative: quote,
               qty: buy ? '0.00000000' : qty, quote: buy ? quote : '0.00000000').merge('symbol' => pair, 'side' => side.upcase)
    else
      kc_order(cb_ext(venue, n, sym), active: status == 'open', cancel: status == 'cancelled', deal_size: filled, deal_funds: quote,
               size: buy ? '0' : qty, funds: buy ? quote : '0').merge('symbol' => pair, 'side' => side, 'fee' => fee)
    end
  end

  # The reads that refresh the swap's orders, in the order the fires ask for them: `orders` is a list of
  # [n, sym, side, status, qty, filled, quote, price, fee, trades]; Binance's trades are [price, qty, commission, asset].
  def cb_order_reads(venue, orders)
    case venue
    when 'kraken'
      { 'POST /0/private/QueryOrders' => [ok(query_body(orders.to_h do |n, sym, side, status, qty, filled, quote, price, fee, _|
        [cb_ext(venue, n, sym), cb_order(venue, n, sym, side, status, qty:, filled:, quote:, price:, fee:)]
      end))] }
    when 'binance'
      trades = orders.reject { |o| o[5].to_d.zero? }.map do |n, sym, side, _, _, _, _, _, _, list|
        ok(list.each_with_index.map do |(price, qty, commission, asset), i|
          bn_trade(i + 1, 28_456 + n, price, qty, commission:, asset:, buyer: side == 'buy').merge('symbol' => cb_pair(venue, sym))
        end)
      end
      { 'GET /api/v3/order' => orders.map { |n, sym, side, status, qty, filled, quote, price, fee, _| ok(cb_order(venue, n, sym, side, status, qty:, filled:, quote:, price:, fee:)) },
        'GET /api/v3/myTrades' => trades.presence || [ok([])] }
    else
      orders.to_h do |n, sym, side, status, qty, filled, quote, price, fee, _|
        ["GET /api/v1/orders/#{cb_ext(venue, n, sym)}", [kc_ok(cb_order(venue, n, sym, side, status, qty:, filled:, quote:, price:, fee:))]]
      end
    end
  end

  def cb_rebalance_scenarios
    fires = (1..3).map { |k| (Time.iso8601(BASKET_STARTED).beginning_of_day + 12.hours + 30.minutes + (4 * (k - 1)).hours).iso8601(6) }
    holdings = { 'w50' => { 'BTC' => '0.01', 'ETH' => '0.04' }, 'tiers' => { 'BTC' => '0.01', 'ETH' => '0.04', 'SOL' => '0.02' } }
    buy_sym = { 'w50' => 'ETH', 'tiers' => 'SOL' }
    owed = { 'rebalance_pending' => { 'phase' => 'buying', 'sell_transaction_id' => nil, 'buy_transaction_id' => nil,
                                      'remaining_quote_amount' => '120.0', 'buy_attempted' => false } }
    unreadable = { 'kraken' => { 'ETH' => kr_ticker('x', 'x', 'x', 'x')['result']['x'] }, 'binance' => { 'ETH' => 'abc' }, 'kucoin' => { 'ETH' => '12x' } }
    raised = { 'kraken' => { 'ETH' => { 'a' => %w[1 1 1] } }, 'binance' => { 'ETH' => { 'symbol' => 'ETHUSDT' } }, 'kucoin' => { 'ETH' => { 'last' => '2500.47' } } }
    five_xx = { 'kraken' => { 'status' => 503, 'body' => { 'error' => ['EService:Unavailable'] } },
                'binance' => bn_fail(502, -1001, 'Internal error; unable to process your request. Please try again.'),
                'kucoin' => { 'status' => 503, 'body' => { 'code' => '500000', 'msg' => 'Internal Server Error' } } }
    no_id = { 'kraken' => ok('error' => [], 'result' => { 'descr' => { 'order' => 'sell' } }), 'binance' => ok('symbol' => 'BTCUSDT', 'orderListId' => -1),
              'kucoin' => kc_ok({}) }
    # A placement the venue refused before trading (Exchange::PLACEMENT_SAFE_TRANSIENT_ERRORS), and one whose outcome it leaves unknown.
    safe = { 'binance' => bn_fail(400, -1021, BN_1021), 'kucoin' => kc_err(400, '400002', 'Invalid KC-API-TIMESTAMP') }
    unknown = { 'kraken' => ok('error' => ['EService:Busy']), 'kucoin' => kc_err(200, '500000', 'Internal Server Error') }
    CB_VENUES.flat_map do |venue|
      bid = ->(sym) { VENUE_PRICES.fetch(venue).fetch(sym)[0] }
      ask = ->(sym) { VENUE_PRICES.fetch(venue).fetch(sym)[1] }
      WEIGHTS.slice('w50', 'tiers').flat_map do |w, weights|
        qty = holdings.fetch(w)
        rows = qty.each_with_index.map { |(sym, q), i| cb_held(venue, i + 1, sym, q) }
        b = buy_sym.fetch(w)
        # The sell: 0.004 BTC at its bid; Binance's proceeds net of a quote commission.
        sell_quote = (BigDecimal('0.004') * bid.('BTC').to_d).round(2).to_s('F')
        sell = lambda do |status, filled = '0.004', quote = sell_quote, fee: '0', trades: nil|
          [1, 'BTC', 'sell', status, '0.004', filled, quote, filled.to_d.zero? ? '0' : bid.('BTC'), fee, trades || [[bid.('BTC'), filled, '0', 'BNB']]]
        end
        buy = lambda do |n, status, quote, fee: '0', trades: nil|
          filled = (quote.to_d / ask.(b).to_d).round(4).to_s('F')
          [n, b, 'buy', status, '0', filled, quote, filled.to_d.zero? ? '0' : ask.(b), fee, trades || [[ask.(b), filled, '0', 'BNB']]]
        end
        half = (sell_quote.to_d / 2).round(2).to_s('F')
        resting_buy = { 'asset' => 'BTC', 'status' => 0, 'external_status' => 1, 'external_id' => cb_ext(venue, 9, 'BTC'), 'order_type' => 1, 'amount' => '0.001',
                        'price' => '60000', 'amount_exec' => '0', 'quote_amount_exec' => '0', 'created_at' => '2026-09-01 10:00:02' }
        states = {
          'swap' => { 'fires' => 3, 'orders' => [sell.('closed'), buy.(2, 'closed', sell_quote)] },
          'under_threshold' => { 'fires' => 1, 'settings' => { 'rebalance_threshold' => 0.5 } },
          'rule_off' => { 'fires' => 1, 'settings' => { 'rebalance_enabled' => false } },
          'selling' => { 'fires' => 1, 'settings' => { 'direction' => 'selling', 'sell_denomination' => 'quote', 'sell_quote_amount' => 10.0 } },
          'resting_sell' => { 'fires' => 2, 'orders' => [sell.('open', '0', '0')] },
          'cancelled_partial_sell' => { 'fires' => 2, 'orders' => [sell.('cancelled', '0.002', (sell_quote.to_d / 2).round(2).to_s('F'))] },
          'nothing_sold' => { 'fires' => 2, 'orders' => [sell.('cancelled', '0', '0')] },
          'partial_buy' => { 'fires' => 3, 'orders' => [sell.('closed'), buy.(2, 'cancelled', half)] },
          'open_buy' => { 'fires' => 1, 'rows' => rows + [resting_buy],
                          'orders' => [[9, 'BTC', 'buy', 'open', '0.001', '0', '0', '0', '0', []]] },
          'sell_below_minimum' => { 'fires' => 1, 'tickers_after' => { 'BTC' => { 'minimum_quote_size' => '5000', 'minimum_base_size' => '1' } } },
          'buy_below_minimum' => { 'fires' => 2, 'orders' => [sell.('closed')], 'quote' => '0.01' },
          'buy_capped' => { 'fires' => 2, 'orders' => [sell.('closed')], 'quote' => '150' },
          'rejected' => { 'fires' => 1, 'http' => { cb_place_key(venue) => [cb_rejected(venue)] } },
          'server_error' => { 'fires' => 1, 'http' => { cb_place_key(venue) => [five_xx.fetch(venue)] } },
          'post_send' => { 'fires' => 1, 'http' => { cb_place_key(venue) => [POST_SEND] } },
          'no_order_id' => { 'fires' => 1, 'http' => { cb_place_key(venue) => [no_id.fetch(venue)] } },
          'owed_buy' => { 'fires' => 1, 'settings' => { 'rebalance_enabled' => false }, 'transient' => owed },
          'unreadable_price' => { 'fires' => 1, 'last' => unreadable.fetch(venue) },
          'raised_price' => { 'fires' => 1, 'last' => raised.fetch(venue) },
          'unpriced_member' => { 'fires' => 1, 'last' => { 'ETH' => nil }, 'unknown' => venue == 'kucoin' ? [] : ['ETH'] }
        }
        # The takeover group: swap states Rails wrote, as its own crash windows and its halt resolution (R0) leave them.
        pending = lambda do |phase, sell: nil, buy: nil, rem: nil, attempted: false|
          { 'rebalance_pending' => { 'phase' => phase, 'sell_transaction_id' => sell, 'buy_transaction_id' => buy, 'remaining_quote_amount' => rem,
                                     'buy_attempted' => attempted } }
        end
        swap_row = lambda do |n, sym, side, status, filled, quote, price|
          { 'asset' => sym, 'status' => 0, 'external_status' => { 'closed' => 2, 'cancelled' => 3 }.fetch(status), 'external_id' => cb_ext(venue, n, sym),
            'order_type' => 0, 'side' => side == 'sell' ? 1 : 0, 'transaction_type' => 'REBALANCE', 'amount' => side == 'sell' ? '0.004' : nil,
            'quote_amount' => side == 'buy' ? '80' : nil, 'amount_exec' => filled, 'quote_amount_exec' => quote, 'price' => price, 'created_at' => '2026-09-01 12:00:00' }
        end
        sell_id = rows.size + 1
        buy_qty = (BigDecimal('40') / ask.(b).to_d).round(4).to_s('F')
        states['takeover_sell_no_id'] = { 'fires' => 1, 'transient' => pending.('selling') }
        states['takeover_buy_attempted_no_id'] = { 'fires' => 1, 'transient' => pending.('buying', rem: '120.0', attempted: true) }
        states['takeover_buy_not_attempted'] = { 'fires' => 1, 'transient' => pending.('buying', rem: '120.0') }
        # R0: a resolved halt that named a cancelled buy which filled 40 of the 80 owed: only the remainder is bought.
        states['takeover_partial_buy'] = { 'fires' => 1, 'rows' => rows + [swap_row.(7, 'BTC', 'sell', 'closed', '0.004', sell_quote, bid.('BTC')),
                                                                           swap_row.(8, b, 'buy', 'cancelled', buy_qty, '40', ask.(b))],
                                           'transient' => pending.('buying', sell: sell_id, buy: sell_id + 1, rem: '80.0', attempted: true),
                                           'orders' => [[8, b, 'buy', 'cancelled', '0', buy_qty, '40', ask.(b), '0', [[ask.(b), buy_qty, '0', 'BNB']]]] }
        # R0: a resolved halt that named a sell which filled: its proceeds are owed to the buy leg.
        states['takeover_resolved_sell'] = { 'fires' => 1, 'rows' => rows + [swap_row.(7, 'BTC', 'sell', 'closed', '0.004', sell_quote, bid.('BTC'))],
                                             'transient' => pending.('selling', sell: sell_id),
                                             'orders' => [[7, 'BTC', 'sell', 'closed', '0.004', '0.004', sell_quote, bid.('BTC'), '0', [[bid.('BTC'), '0.004', '0', 'BNB']]]] }
        # A closed sell whose proceeds were never read, and the refresh fails: the swap waits for them, and trades nothing.
        read_fails = { 'kraken' => { 'POST /0/private/QueryOrders' => [ok('error' => ['EService:Unavailable'])] },
                       'binance' => { 'GET /api/v3/order' => [bn_fail(503, -1001, 'Internal error; unable to process your request. Please try again.')] },
                       'kucoin' => { "GET /api/v1/orders/#{cb_ext(venue, 7, 'BTC')}" => [kc_err(503, '500000', 'Internal Server Error')] } }
        states['takeover_proceeds_unknown'] = { 'fires' => 1, 'rows' => rows + [swap_row.(7, 'BTC', 'sell', 'closed', nil, nil, nil)],
                                                'transient' => pending.('selling', sell: sell_id), 'http' => read_fails.fetch(venue) }
        states['safe_rejection'] = { 'fires' => 1, 'http' => { cb_place_key(venue) => [safe.fetch(venue)] } } if safe.key?(venue)
        states['unknown_text'] = { 'fires' => 1, 'http' => { cb_place_key(venue) => [unknown.fetch(venue)] } } if unknown.key?(venue)
        # Fees: the owed buy is the sell's recorded proceeds (gross on Kraken and KuCoin, net of a Binance quote commission);
        # a partial buy owes the remainder of what it recorded.
        fee_cases = if venue == 'binance'
                      { 'quote' => ['0.27', 'USDT'], 'base' => ['0.000004', 'BTC'], 'bnb' => ['0.0005', 'BNB'] }
                    else { 'quote' => ['0.27', CB_QUOTE.fetch(venue)] }
                    end
        fee_cases.each do |kind, (c, asset)|
          sell_trades = [[bid.('BTC'), '0.004', c, asset]]
          buy_c = asset == 'BTC' ? '0.00001' : c
          buy_asset = asset == 'BTC' ? b : asset
          half_qty = (half.to_d / ask.(b).to_d).round(4).to_s('F')
          fee = venue == 'binance' ? '0' : c
          states["fee_#{kind}_swap"] = { 'fires' => 2, 'orders' => [sell.('closed', fee:, trades: sell_trades)] }
          states["fee_#{kind}_partial_buy"] = { 'fires' => 3, 'orders' => [sell.('closed', fee:, trades: sell_trades),
                                                                         buy.(2, 'cancelled', half, fee:, trades: [[ask.(b), half_qty, buy_c, buy_asset]])] }
        end
        states.map do |state, st|
          http = cb_http(venue, qty, last: st.fetch('last', {}), unknown: st.fetch('unknown', []), quote: st.fetch('quote', '100000'))
                 .merge(cb_order_reads(venue, st.fetch('orders', [])), st.fetch('http', {}))
          { 'name' => "rebalance-#{venue}-#{w}-#{state}", 'venue' => venue, 'interval' => 'day', 'quote_amount' => 60.0, 'started_at' => BASKET_STARTED,
            'settings' => { 'rebalance_enabled' => true }.merge(st.fetch('settings', {})), 'settings_changed_at' => nil, 'transient' => st.fetch('transient', {}),
            'transactions' => st.fetch('rows', rows), 'ticker' => VENUE_PAIRS.fetch(venue)['BTC'], 'members' => weights, 'at' => fires.first,
            'script' => { CB_SCRIPT.fetch(venue) => http }, 'tick' => false, 'rebalance_at' => fires.first(st.fetch('fires')),
            'tickers_after' => st['tickers_after'], 'bot_columns' => { 'status' => Bot.statuses[:stopped] } }
        end
      end
    end
  end

  # A stopped index bot whose CCC left the index (in_index false, 0.5 still held) and stays out (top_coins AAA BBB DDD): the
  # exited holding is outside the arithmetic, so the drift is DDD's shortfall among AAA, BBB and DDD, and AAA is sold.
  def index_rebalance_scenario
    fire = '2026-09-08T16:30:00.000000Z'
    prices = INDEX_SYMBOLS.zip([100, 200, 300, 400, 500]).to_h
    held = [%w[AAA 1.0], %w[BBB 0.3], %w[CCC 0.5], %w[DDD 0.01]]
    pos = held.map { |sym, q| { 'symbol' => sym, 'asset_class' => 'us_equity', 'qty' => q, 'qty_available' => q } }
    http = { 'GET /v2/clock' => [open_clock(fire)], 'GET /v2/account' => [stock_account('100000', '200000')], 'GET /v2/positions' => [ok(pos)],
             'GET /v2/stocks/snapshots' => [ok(prices.transform_values { |p| { 'latestTrade' => { 'p' => p } } })],
             'POST /v2/orders' => [ok(stock_order('OTX-1', 'new', sym: 'AAA', notional: nil).merge('side' => 'sell'))] }
    prices.each do |sym, p|
      http["GET /v2/stocks/#{sym}/quotes/latest"] = [stock_quote(sym, p)]
      http["GET /v2/stocks/#{sym}/trades/latest"] = [stock_trade(sym, p)]
    end
    member = ->(sym, target, in_index: true) { { 'symbol' => sym, 'target' => target, 'in_index' => in_index } }
    { 'name' => 'rebalance-index-exited_member', 'venue' => 'alpaca', 'kind' => 'index', 'at' => fire, 'top_coins' => %w[AAA BBB DDD],
      'members' => [member.('AAA', 0.5), member.('BBB', 0.3), member.('CCC', 0.2, in_index: false), member.('DDD', 0.2)],
      'transactions' => held, 'num_coins' => 3, 'flattening' => 0.0, 'hold_all' => false, 'settings' => { 'rebalance_enabled' => true },
      'bot_columns' => { 'status' => Bot.statuses[:stopped] }, 'script' => { 'alpaca' => http }, 'tick' => false, 'rebalance_at' => [fire] }
  end

  # `venue`: 'binance', or 'binance_us' (Exchanges::BinanceUs, the same code at api.binance.us: plan VB slice A2).
  def binance_scenarios(venue = 'binance')
    started = '2026-09-01T10:00:00.123456Z'
    ticker = { 'base_decimals' => 5, 'quote_decimals' => 8, 'price_decimals' => 2, 'minimum_base_size' => '0.00001', 'minimum_quote_size' => '5' }
    http = {
      'GET /api/v3/ticker/bookTicker' => [bn_book('64000.11000000')],
      'GET /api/v3/ticker/price' => [bn_last('64000.05000000')],
      'POST /api/v3/order' => [ok('symbol' => 'BTCUSDT', 'orderId' => 28_457, 'orderListId' => -1, 'clientOrderId' => 'x', 'transactTime' => 1_788_000_000_000)],
      'GET /api/v3/account' => [bn_account('100000.00000000')],
      'GET /api/v3/allOrders' => [ok([])],
      'GET /api/v3/myTrades' => [ok([])]
    }
    modes = { 'market' => {}, 'limit' => { 'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.0025 } }
    closed = { 'status' => 0, 'external_status' => 2, 'external_id' => 'BTCUSDT-100', 'order_type' => 0, 'quote_amount' => '60',
               'quote_amount_exec' => '60', 'amount_exec' => '0.00093', 'price' => '64000', 'created_at' => '2026-09-01 10:00:01' }
    waiting = lambda do |id, limit: false|
      { 'status' => 0, 'external_status' => limit ? 1 : 0, 'external_id' => "BTCUSDT-#{id}", 'order_type' => limit ? 1 : 0,
        'quote_amount' => limit ? nil : '60', 'amount' => limit ? '0.00093' : nil, 'price' => '64000', 'amount_exec' => '0',
        'quote_amount_exec' => '0', 'created_at' => '2026-09-01 10:00:01' }.compact
    end
    filled = ->(id, trades) { { 'GET /api/v3/allOrders' => [ok([bn_order(id, 'FILLED', executed: trades.sum { |t| BigDecimal(t['qty']) }.to_s('F'), cumulative: '60.00000000')])],
                                'GET /api/v3/myTrades' => [ok(trades)] } }
    %w[day week].flat_map do |interval|
      step = { 'day' => 1.day, 'week' => 1.week }.fetch(interval)
      after = ->(n) { (Time.iso8601(started) + (n * step) + 1.second).iso8601(6) }
      variants = {
        'first_tick' => { 'at' => (Time.iso8601(started) + 0.5).iso8601(6) },
        'on_schedule' => { 'at' => after.(1), 'transactions' => [closed] },
        'carry' => { 'at' => after.(1), 'transient' => { 'missed_quote_amount' => '12.5' } },
        'below_minimum' => { 'at' => after.(1), 'quote_amount' => 0.4 },
        # A market buy sized in base: LOT_SIZE's minimum in quote is above NOTIONAL's (Bot::OrderSetter :base_and_quote).
        'base_sized' => { 'at' => after.(1), 'ticker' => ticker.merge('base_decimals' => 3, 'minimum_base_size' => '0.001', 'minimum_quote_size' => '1') },
        'notional_decides' => { 'at' => after.(1), 'ticker' => ticker.merge('minimum_quote_size' => '100') },
        'lot_size_decides' => { 'at' => after.(1), 'ticker' => ticker.merge('base_decimals' => 2, 'minimum_base_size' => '0.01') },
        'minus_1021' => { 'at' => after.(1), 'http' => { 'POST /api/v3/order' => [bn_fail(400, -1021, BN_1021)] } },
        'minus_1021_ahead' => { 'at' => after.(1), 'http' => { 'POST /api/v3/order' => [bn_fail(400, -1021, BN_1021_AHEAD)] } },
        'insufficient' => { 'at' => after.(1), 'http' => { 'POST /api/v3/order' => [bn_fail(400, -2010, 'Account has insufficient balance for requested action.')] } },
        'filter_failure' => { 'at' => after.(1), 'http' => { 'POST /api/v3/order' => [bn_fail(400, -1013, 'Filter failure: NOTIONAL')] } },
        'invalid_key' => { 'at' => after.(1), 'transient' => { 'last_failure_kind' => 'invalid_key' },
                           'http' => { 'POST /api/v3/order' => [bn_fail(401, -2015, 'Invalid API-key, IP, or permissions for action.')] } },
        # Sanctioned divergences (rust/tests/parity.rs BINANCE_DIVERGENCES): Rails writes a failed row (or a "BTCUSDT-" row),
        # Rust keeps the intent and settles it by client order id, since Binance may have placed the order.
        'add_server_error' => { 'at' => after.(1), 'http' => { 'POST /api/v3/order' => [bn_fail(503, -1001, 'Internal error; unable to process your request. Please try again.')] } },
        'add_timeout_after_send' => { 'at' => after.(1), 'http' => { 'POST /api/v3/order' => [POST_SEND] } },
        'add_no_order_id' => { 'at' => after.(1), 'http' => { 'POST /api/v3/order' => [ok('symbol' => 'BTCUSDT')] } },
        'add_unreadable' => { 'at' => after.(1), 'http' => { 'POST /api/v3/order' => [{ 'status' => 200, 'body' => '<html>upstream</html>' }] } },
        'add_network_pre_send' => { 'at' => after.(1), 'http' => { 'POST /api/v3/order' => [VENUES.fetch(venue).not_sent] } },
        'low_funds' => { 'at' => after.(1), 'http' => { 'GET /api/v3/account' => [bn_account('1.00000000')] } },
        'zero_price' => { 'at' => after.(1), 'http' => { 'GET /api/v3/ticker/bookTicker' => [bn_book('0.00000000')], 'GET /api/v3/ticker/price' => [bn_last('0.00000000')] } },
        'no_price' => { 'at' => after.(1), 'http' => { 'GET /api/v3/ticker/bookTicker' => [bn_fail(400, -1121, 'Invalid symbol.')],
                                                       'GET /api/v3/ticker/price' => [bn_fail(400, -1121, 'Invalid symbol.')] } },
        # The tick's sweep (Bot::FetchAndUpdateOpenOrdersJob) over waiting orders: fills rebuilt from their trades.
        'sweep_filled_base_commission' => { 'at' => after.(1), 'transactions' => [waiting.(201)],
          'http' => filled.(201, [bn_trade(1, 201, '64000.11', '0.00060', commission: '0.0000006', asset: 'BTC'), bn_trade(2, 201, '64001.00', '0.00033', commission: '0.00000033', asset: 'BTC')]) },
        'sweep_filled_quote_commission' => { 'at' => after.(1), 'transactions' => [waiting.(202)],
          'http' => filled.(202, [bn_trade(1, 202, '64000.11', '0.00093', commission: '0.0595', asset: 'USDT')]) },
        'sweep_filled_bnb_commission' => { 'at' => after.(1), 'transactions' => [waiting.(203)],
          'http' => filled.(203, [bn_trade(1, 203, '64000.11', '0.00093', commission: '0.00009', asset: 'BNB')]) },
        'sweep_rounding_two_decimals' => { 'at' => after.(1), 'transactions' => [waiting.(204)],
          'http' => filled.(204, [bn_trade(1, 204, '1.00', '1'), bn_trade(2, 204, '1.01', '1'), bn_trade(3, 204, '1.00', '1')]) },
        'sweep_partially_filled' => { 'at' => after.(1), 'transactions' => [waiting.(205, limit: true)],
          'http' => { 'GET /api/v3/allOrders' => [ok([bn_order(205, 'PARTIALLY_FILLED', type: 'LIMIT', executed: '0.00040000', cumulative: '25.60000000')])],
                      'GET /api/v3/myTrades' => [ok([bn_trade(1, 205, '64000.00', '0.00040', commission: '0.0000004', asset: 'BTC')])] } },
        'sweep_expired_partial' => { 'at' => after.(1), 'transactions' => [waiting.(206)],
          'http' => { 'GET /api/v3/allOrders' => [ok([bn_order(206, 'EXPIRED', executed: '0.00050000', cumulative: '32.00000000')])],
                      'GET /api/v3/myTrades' => [ok([bn_trade(1, 206, '64000.00', '0.00050', commission: '0.032', asset: 'USDT')])] } },
        'sweep_rejected' => { 'at' => after.(1), 'transactions' => [waiting.(207)], 'http' => { 'GET /api/v3/allOrders' => [ok([bn_order(207, 'REJECTED')])] } },
        'sweep_pending_cancel' => { 'at' => after.(1), 'transactions' => [waiting.(208)], 'http' => { 'GET /api/v3/allOrders' => [ok([bn_order(208, 'PENDING_CANCEL')])] } },
        'sweep_new_unfilled' => { 'at' => after.(1), 'transactions' => [waiting.(209, limit: true)],
          'http' => { 'GET /api/v3/allOrders' => [ok([bn_order(209, 'NEW', type: 'LIMIT')])] } },
        'sweep_not_returned_waits' => { 'at' => after.(1), 'transactions' => [waiting.(210).merge('created_at' => '2026-08-01 10:00:01')] },
        'sweep_historical_negative_quote' => { 'at' => after.(1), 'transactions' => [waiting.(211)],
          'http' => { 'GET /api/v3/allOrders' => [ok([bn_order(211, 'CANCELED', cumulative: '-1.00000000')])] } },
        # Five orders with executions in one sweep, each rebuilt from its own trades.
        'sweep_five_executed' => { 'at' => after.(1), 'transactions' => (301..305).map { |i| waiting.(i) },
          'http' => { 'GET /api/v3/allOrders' => [ok((301..305).map { |i| bn_order(i, 'FILLED', executed: '0.00093000', cumulative: '59.52000000') })],
                      'GET /api/v3/myTrades' => (301..305).map { |i| ok([bn_trade(i, i, '64000.00', '0.00093', commission: '0.0000009', asset: 'BTC')]) } } },
        # More than 1000 trades: a full page, then the next from the highest id + 1; Binance serves trade 1000 again.
        'sweep_trades_over_a_page' => { 'at' => after.(1), 'transactions' => [waiting.(212)],
          'http' => { 'GET /api/v3/allOrders' => [ok([bn_order(212, 'FILLED', executed: '0.00100100', cumulative: '64.06406400')])],
                      'GET /api/v3/myTrades' => [ok((1..1000).map { |i| bn_trade(i, 212, '64000.00', '0.000001') }),
                                                 ok([bn_trade(1000, 212, '64000.00', '0.000001'), bn_trade(1001, 212, '64064.00', '0.000001')])] } },
        'sweep_incomplete_walk' => { 'at' => after.(1), 'transactions' => [waiting.(213)],
          'http' => filled.(213, [bn_trade(1, 213, '64000.00', '0.0005')]).merge('GET /api/v3/allOrders' => [ok([bn_order(213, 'FILLED', executed: '0.00093000', cumulative: '59.52')])]) },
        # Listed divergence (spec E §2.10): HTTP 429 on a read waits as a throttle in Rust; Rails fails the run.
        'sweep_rate_limited' => { 'at' => after.(1), 'transactions' => [waiting.(214)],
          'http' => { 'GET /api/v3/allOrders' => [bn_fail(429, -1003, 'Too much request weight used; current limit is 6000 request weight per 1 MINUTE.')] } },
        # Poll-only: Bot::FetchAndUpdateOrderJob (#get_order) for one waiting order.
        'poll_filled' => { 'at' => after.(1), 'tick' => false, 'poll' => 'BTCUSDT-401', 'transactions' => [waiting.(401)],
          'http' => { 'GET /api/v3/order' => [ok(bn_order(401, 'FILLED', executed: '0.00093000', cumulative: '59.52000000'))],
                      'GET /api/v3/myTrades' => [ok([bn_trade(1, 401, '64000.00', '0.00093', commission: '0.0595', asset: 'USDT')])] } },
        'poll_open' => { 'at' => after.(1), 'tick' => false, 'poll' => 'BTCUSDT-402', 'transactions' => [waiting.(402, limit: true)],
          'http' => { 'GET /api/v3/order' => [ok(bn_order(402, 'NEW', type: 'LIMIT'))] } },
        'poll_pending_cancel' => { 'at' => after.(1), 'tick' => false, 'poll' => 'BTCUSDT-403', 'transactions' => [waiting.(403)],
          'http' => { 'GET /api/v3/order' => [ok(bn_order(403, 'PENDING_CANCEL'))] } },
        'poll_not_found' => { 'at' => after.(1), 'tick' => false, 'poll' => 'BTCUSDT-404', 'transactions' => [waiting.(404)],
          'http' => { 'GET /api/v3/order' => [bn_fail(400, -2013, 'Order does not exist.')] } },
        # A tick that places BTCUSDT-28457; its follow-up 5 s later finds it filled.
        'tick_then_poll' => { 'at' => after.(1), 'poll' => 'BTCUSDT-28457', 'transient' => { 'missed_quote_amount' => '12.5' },
          'http' => { 'GET /api/v3/order' => [ok(bn_order(28_457, 'FILLED', executed: '0.00112000', cumulative: '71.68000000'))],
                      'GET /api/v3/myTrades' => [ok([bn_trade(1, 28_457, '64000.00', '0.00112', commission: '0.00000112', asset: 'BTC')])] } }
      }
      # Listed divergences (rust/tests/parity.rs UNREADABLE): a number Rust cannot read in a price, and in a sweep's order.
      { 'nan' => 'NaN', 'garbage' => 'garbage' }.each do |label, bad|
        variants["unreadable_price_#{label}"] = { 'at' => after.(1), 'http' => { 'GET /api/v3/ticker/bookTicker' => [bn_book(bad)], 'GET /api/v3/ticker/price' => [bn_last(bad)] } }
        variants["unreadable_poll_#{label}"] = { 'at' => after.(1), 'transactions' => [waiting.(215)],
          'http' => { 'GET /api/v3/allOrders' => [ok([bn_order(215, 'FILLED', executed: bad, cumulative: '60')])] } }
      end
      modes.flat_map do |mode, settings|
        variants.map do |name, v|
          { 'name' => "#{interval}-#{mode}-#{name}", 'venue' => venue, 'interval' => interval, 'quote_amount' => v.fetch('quote_amount', 60.0),
            'started_at' => started, 'settings' => settings, 'settings_changed_at' => v['settings_changed_at'],
            'transient' => v.fetch('transient', {}), 'transactions' => v.fetch('transactions', []), 'ticker' => v.fetch('ticker', ticker),
            'ticker_after' => v['ticker_after'], 'at' => v.fetch('at'), 'script' => { venue => http.merge(v.fetch('http', {})) },
            'tick' => v.fetch('tick', true), 'poll' => v['poll'] }
        end
      end
    end
  end

  # KuCoin spot (plan K5), one asset on BTC-USDT: Rails' own answers to the bodies below, beneath honeymaker.
  def kc_ok(data) = ok('code' => '200000', 'data' => data)
  def kc_err(status, code, msg) = { 'status' => status, 'body' => { 'code' => code, 'msg' => msg } }
  def kc_book(ask, bid: '64000.1') = kc_ok('time' => 1_788_000_000_000, 'sequence' => '1', 'bids' => [[bid, '1.5']], 'asks' => [[ask, '0.2']])
  def kc_tickers(last) = kc_ok('time' => 1_788_000_000_000, 'ticker' => [{ 'symbol' => 'ETH-USDT', 'last' => '2500' }, { 'symbol' => 'BTC-USDT', 'last' => last }])
  def kc_accounts(usdt) = kc_ok([{ 'id' => 'a1', 'currency' => 'BTC', 'type' => 'trade', 'balance' => '0.5', 'available' => '0.5', 'holds' => '0' },
                                 { 'id' => 'a2', 'currency' => 'USDT', 'type' => 'trade', 'balance' => usdt, 'available' => usdt, 'holds' => '0' }])
  def kc_order(id, active:, cancel: false, type: 'market', deal_size: '0', deal_funds: '0', price: '0', size: '0', funds: '60')
    { 'id' => id, 'symbol' => 'BTC-USDT', 'opType' => 'DEAL', 'type' => type, 'side' => 'buy', 'price' => price, 'size' => size, 'funds' => funds,
      'dealFunds' => deal_funds, 'dealSize' => deal_size, 'fee' => '0', 'feeCurrency' => 'USDT', 'clientOid' => "web-#{id}",
      'isActive' => active, 'cancelExist' => cancel, 'cancelReason' => '', 'createdAt' => 1_788_000_000_000 }
  end
  def kc_fills(items) = kc_ok('currentPage' => 1, 'pageSize' => 500, 'totalNum' => items.size, 'totalPage' => items.empty? ? 0 : 1, 'items' => items)
  def kc_fill(trade, order, size, funds) = { 'symbol' => 'BTC-USDT', 'tradeId' => trade, 'orderId' => order, 'side' => 'buy', 'type' => 'market', 'price' => '64000',
                                             'size' => size, 'funds' => funds, 'fee' => '0.06', 'feeCurrency' => 'USDT', 'createdAt' => 1_788_000_000_000 }
  KC_NOT_FOUND = { 'status' => 404, 'body' => { 'code' => '400100', 'msg' => 'order not exist.' } }.freeze

  def kucoin_scenarios
    started = '2026-09-01T10:00:00.123456Z'
    ticker = { 'base_decimals' => 8, 'quote_decimals' => 6, 'price_decimals' => 1, 'minimum_base_size' => '0.00001', 'minimum_quote_size' => '0.1' }
    http = {
      'GET /api/v1/market/orderbook/level2_20' => [kc_book('64000.2')],
      'GET /api/v1/market/allTickers' => [kc_tickers('64000.05')],
      'POST /api/v1/orders' => [kc_ok('orderId' => '6716f0a2c1b6d10007a3b1c5')],
      'GET /api/v1/accounts' => [kc_accounts('100000')],
      'GET /api/v1/fills' => [kc_fills([])]
    }
    modes = { 'market' => {}, 'limit' => { 'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.0025 } }
    closed = { 'status' => 0, 'external_status' => 2, 'external_id' => 'KC-100', 'order_type' => 0, 'quote_amount' => '60',
               'quote_amount_exec' => '60', 'amount_exec' => '0.00093', 'price' => '64000', 'created_at' => '2026-09-01 10:00:01' }
    waiting = lambda do |id, limit: false, created: '2026-09-01 10:00:01'|
      { 'status' => 0, 'external_status' => limit ? 1 : 0, 'external_id' => "KC-#{id}", 'order_type' => limit ? 1 : 0,
        'quote_amount' => limit ? nil : '60', 'amount' => limit ? '0.00093' : nil, 'price' => '64000', 'amount_exec' => '0',
        'quote_amount_exec' => '0', 'created_at' => created }.compact
    end
    polled = ->(id, order) { { "GET /api/v1/orders/KC-#{id}" => [kc_ok(order.merge('id' => "KC-#{id}"))] } }
    %w[day week].flat_map do |interval|
      step = { 'day' => 1.day, 'week' => 1.week }.fetch(interval)
      after = ->(n) { (Time.iso8601(started) + (n * step) + 1.second).iso8601(6) }
      variants = {
        'first_tick' => { 'at' => (Time.iso8601(started) + 0.5).iso8601(6) },
        'on_schedule' => { 'at' => after.(1), 'transactions' => [closed] },
        'carry' => { 'at' => after.(1), 'transient' => { 'missed_quote_amount' => '12.5' } },
        'below_minimum' => { 'at' => after.(1), 'quote_amount' => 0.05 },
        'base_sized' => { 'at' => after.(1), 'ticker' => ticker.merge('base_decimals' => 3, 'minimum_base_size' => '0.001', 'minimum_quote_size' => '1') },
        'quote_min_decides' => { 'at' => after.(1), 'ticker' => ticker.merge('minimum_quote_size' => '100') },
        'base_min_decides' => { 'at' => after.(1), 'ticker' => ticker.merge('base_decimals' => 2, 'minimum_base_size' => '0.01') },
        # 400002 is placement-safe: no row, the run rescheduled; in an HTTP-200 envelope and as a 400 alike.
        'timestamp_envelope' => { 'at' => after.(1), 'http' => { 'POST /api/v1/orders' => [ok('code' => '400002', 'msg' => 'Invalid KC-API-TIMESTAMP')] } },
        'timestamp_400' => { 'at' => after.(1), 'http' => { 'POST /api/v1/orders' => [kc_err(400, '400002', 'Invalid KC-API-TIMESTAMP')] } },
        'insufficient' => { 'at' => after.(1), 'http' => { 'POST /api/v1/orders' => [ok('code' => '200004', 'msg' => 'Balance insufficient!')] } },
        'below_venue_minimum' => { 'at' => after.(1), 'http' => { 'POST /api/v1/orders' => [kc_err(400, '400100', 'Order size below the minimum requirement.')] } },
        'invalid_key' => { 'at' => after.(1), 'transient' => { 'last_failure_kind' => 'invalid_key' },
                           'http' => { 'POST /api/v1/orders' => [kc_err(401, '400003', 'KC-API-KEY not exists')] } },
        'bad_passphrase' => { 'at' => after.(1), 'http' => { 'POST /api/v1/orders' => [kc_err(401, '400004', 'Invalid KC-API-PASSPHRASE')] } },
        'bad_signature' => { 'at' => after.(1), 'http' => { 'POST /api/v1/orders' => [kc_err(401, '400005', 'Signature error')] } },
        'access_denied' => { 'at' => after.(1), 'http' => { 'POST /api/v1/orders' => [kc_err(400, '400007', 'Access Denied')] } },
        'throttled' => { 'at' => after.(1), 'http' => { 'POST /api/v1/orders' => [kc_err(429, '429000', 'Too Many Requests')] } },
        # Sanctioned divergences (rust/tests/parity.rs KUCOIN_DIVERGENCES): Rust keeps the intent, and on KuCoin it waits for
        # the operator (plan K r4): a lost reply, a 5xx, an unreadable answer, a 200000 without an orderId, 500000 and 230005.
        'add_server_error' => { 'at' => after.(1), 'http' => { 'POST /api/v1/orders' => [{ 'status' => 503, 'body' => '<html>upstream</html>' }] } },
        'add_timeout_after_send' => { 'at' => after.(1), 'http' => { 'POST /api/v1/orders' => [POST_SEND] } },
        'add_no_order_id' => { 'at' => after.(1), 'http' => { 'POST /api/v1/orders' => [kc_ok({})] } },
        'add_unreadable' => { 'at' => after.(1), 'http' => { 'POST /api/v1/orders' => [{ 'status' => 200, 'body' => '<html>upstream</html>' }] } },
        'add_internal_500000' => { 'at' => after.(1), 'http' => { 'POST /api/v1/orders' => [ok('code' => '500000', 'msg' => 'Internal Server Error')] } },
        'add_busy_230005' => { 'at' => after.(1), 'http' => { 'POST /api/v1/orders' => [ok('code' => '230005', 'msg' => 'The system is busy, please try again later.')] } },
        'add_network_pre_send' => { 'at' => after.(1), 'http' => { 'POST /api/v1/orders' => [VENUES.fetch('kucoin').not_sent] } },
        'low_funds' => { 'at' => after.(1), 'http' => { 'GET /api/v1/accounts' => [kc_accounts('1')] } },
        'balance_refused' => { 'at' => after.(1), 'http' => { 'GET /api/v1/accounts' => [ok('code' => '400003', 'msg' => 'KC-API-KEY not exists')] } },
        'zero_price' => { 'at' => after.(1), 'http' => { 'GET /api/v1/market/orderbook/level2_20' => [kc_book('0', bid: '0')], 'GET /api/v1/market/allTickers' => [kc_tickers('0')] } },
        'no_price' => { 'at' => after.(1), 'http' => { 'GET /api/v1/market/orderbook/level2_20' => [ok('code' => '400100', 'msg' => 'symbol not exists')],
                                                       'GET /api/v1/market/allTickers' => [ok('code' => '400100', 'msg' => 'symbol not exists')] } },
        # The tick's sweep (Bot::FetchAndUpdateOpenOrdersJob): #get_order per waiting order.
        'sweep_done_dealt' => { 'at' => after.(1), 'transactions' => [waiting.(201)], 'http' => polled.(201, kc_order('x', active: false, deal_size: '0.00093', deal_funds: '59.5203')) },
        'sweep_active' => { 'at' => after.(1), 'transactions' => [waiting.(202, limit: true)],
          'http' => polled.(202, kc_order('x', active: true, type: 'limit', price: '64000', size: '0.00093', funds: '0')) },
        'sweep_active_partial' => { 'at' => after.(1), 'transactions' => [waiting.(203, limit: true)],
          'http' => polled.(203, kc_order('x', active: true, type: 'limit', price: '64000', size: '0.00093', funds: '0', deal_size: '0.0004', deal_funds: '25.6')) },
        'sweep_cancelled_partial' => { 'at' => after.(1), 'transactions' => [waiting.(204, limit: true)],
          'http' => polled.(204, kc_order('x', active: false, cancel: true, type: 'limit', price: '64000', size: '0.00093', funds: '0', deal_size: '0.0004', deal_funds: '25.6')) },
        'sweep_cancelled_nothing' => { 'at' => after.(1), 'transactions' => [waiting.(205)], 'http' => polled.(205, kc_order('x', active: false, cancel: true)) },
        'sweep_done_no_deal' => { 'at' => after.(1), 'transactions' => [waiting.(206)], 'http' => polled.(206, kc_order('x', active: false)) },
        'sweep_rounding' => { 'at' => after.(1), 'transactions' => [waiting.(207)], 'http' => polled.(207, kc_order('x', active: false, deal_size: '0.0003', deal_funds: '19.20005')) },
        'sweep_unknown_type' => { 'at' => after.(1), 'transactions' => [waiting.(208)], 'http' => polled.(208, kc_order('x', active: true, type: 'stop')) },
        # A forgotten order: settled from its fills, or left to Bot::StaleOrderResolver (young waits, old is abandoned).
        'sweep_forgotten_filled' => { 'at' => after.(1), 'transactions' => [waiting.(209)],
          'http' => { 'GET /api/v1/orders/KC-209' => [KC_NOT_FOUND], 'GET /api/v1/fills' => [kc_fills([kc_fill('t1', 'KC-209', '0.0006', '38.4'), kc_fill('t2', 'KC-209', '0.0003', '19.2')])] } },
        'sweep_forgotten_young' => { 'at' => after.(1), 'transactions' => [waiting.(210)], 'http' => { 'GET /api/v1/orders/KC-210' => [KC_NOT_FOUND] } },
        'sweep_forgotten_old' => { 'at' => after.(1), 'transactions' => [waiting.(211, created: '2026-08-01 10:00:01')],
          'http' => { 'GET /api/v1/orders/KC-211' => [{ 'status' => 200, 'body' => { 'code' => '400100', 'msg' => 'The order does not exist.' } }] } },
        'sweep_not_found_by_code_only' => { 'at' => after.(1), 'transactions' => [waiting.(212)],
          'http' => { 'GET /api/v1/orders/KC-212' => [kc_err(400, '400100', 'Parameter error')] } },
        # Poll-only: Bot::FetchAndUpdateOrderJob (#get_order) for one waiting order.
        'poll_filled' => { 'at' => after.(1), 'tick' => false, 'poll' => 'KC-401', 'transactions' => [waiting.(401)],
          'http' => polled.(401, kc_order('x', active: false, deal_size: '0.00093', deal_funds: '59.52')) },
        'poll_open' => { 'at' => after.(1), 'tick' => false, 'poll' => 'KC-402', 'transactions' => [waiting.(402, limit: true)],
          'http' => polled.(402, kc_order('x', active: true, type: 'limit', price: '64000', size: '0.00093', funds: '0')) },
        'poll_done_no_deal' => { 'at' => after.(1), 'tick' => false, 'poll' => 'KC-403', 'transactions' => [waiting.(403)], 'http' => polled.(403, kc_order('x', active: false)) },
        # A tick that places 6716f0a2c1b6d10007a3b1c5; its follow-up 5 s later finds it filled.
        'tick_then_poll' => { 'at' => after.(1), 'poll' => '6716f0a2c1b6d10007a3b1c5', 'transient' => { 'missed_quote_amount' => '12.5' },
          'http' => { 'GET /api/v1/orders/6716f0a2c1b6d10007a3b1c5' => [kc_ok(kc_order('6716f0a2c1b6d10007a3b1c5', active: false, deal_size: '0.00112', deal_funds: '71.68'))] } }
      }
      modes.flat_map do |mode, settings|
        variants.map do |name, v|
          { 'name' => "#{interval}-#{mode}-#{name}", 'venue' => 'kucoin', 'interval' => interval, 'quote_amount' => v.fetch('quote_amount', 60.0),
            'started_at' => started, 'settings' => settings, 'settings_changed_at' => v['settings_changed_at'],
            'transient' => v.fetch('transient', {}), 'transactions' => v.fetch('transactions', []), 'ticker' => v.fetch('ticker', ticker),
            'ticker_after' => v['ticker_after'], 'at' => v.fetch('at'), 'script' => { 'kucoin' => http.merge(v.fetch('http', {})) },
            'tick' => v.fetch('tick', true), 'poll' => v['poll'] }
        end
      end
    end
  end

  # ---- Bitvavo (plan VB slice A3): bodies as api.bitvavo.com answers them (AUDIT-bitvavo), errors as {errorCode, error} ----
  def bv_err(status, code, error) = { 'status' => status, 'body' => { 'errorCode' => code, 'error' => error } }
  def bv_book(ask, bid: '64000') = ok('market' => 'BTC-EUR', 'bid' => bid, 'bidSize' => '1.5', 'ask' => ask, 'askSize' => '0.2')
  def bv_last(price) = ok('market' => 'BTC-EUR', 'price' => price)
  def bv_balance(eur) = ok([{ 'symbol' => 'BTC', 'available' => '0.5', 'inOrder' => '0' }, { 'symbol' => 'EUR', 'available' => eur, 'inOrder' => '0' }])
  def bv_order(id, status, type: 'market', side: 'buy', price: nil, amount: nil, amount_quote: nil, filled: '0', filled_quote: '0')
    { 'orderId' => id, 'market' => 'BTC-EUR', 'created' => 1_788_000_000_000, 'updated' => 1_788_000_000_000, 'status' => status, 'side' => side,
      'orderType' => type, 'amount' => amount, 'amountRemaining' => amount, 'amountQuote' => amount_quote, 'price' => price, 'onHold' => '0',
      'onHoldCurrency' => 'EUR', 'filledAmount' => filled, 'filledAmountQuote' => filled_quote, 'feePaid' => '0', 'feeCurrency' => 'EUR',
      'fills' => [], 'selfTradePrevention' => 'decrementAndCancel', 'visible' => true, 'timeInForce' => 'GTC', 'postOnly' => false }.compact
  end
  def bv_trade(id, order, amount, price, at, side: 'buy') = { 'id' => id, 'orderId' => order, 'timestamp' => (Time.iso8601(at).to_r * 1000).to_i,
                                                             'market' => 'BTC-EUR', 'side' => side, 'amount' => amount, 'price' => price, 'taker' => true,
                                                             'fee' => '0.15', 'feeCurrency' => 'EUR', 'settled' => true }
  BV_NOT_FOUND = { 'status' => 404, 'body' => { 'errorCode' => 240, 'error' => 'No order found. Please be aware that simultaneously updating the same order may return this error.' } }.freeze
  BV_ORDER = '0b6d2f7e-1c4a-4b8e-9f3a-5d2c7e1a9b01'.freeze

  def bitvavo_scenarios
    started = '2026-09-01T10:00:00.123456Z'
    ticker = { 'base_decimals' => 8, 'quote_decimals' => 2, 'price_decimals' => 0, 'minimum_base_size' => '0.0001', 'minimum_quote_size' => '5' }
    http = {
      'GET /v2/ticker/book' => [bv_book('64001')],
      'GET /v2/ticker/price' => [bv_last('64000')],
      'POST /v2/order' => [ok(bv_order(BV_ORDER, 'new', amount_quote: '60'))],
      'GET /v2/balance' => [bv_balance('100000')],
      'GET /v2/trades' => [ok([])]
    }
    modes = { 'market' => {}, 'limit' => { 'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.0025 } }
    closed = { 'status' => 0, 'external_status' => 2, 'external_id' => 'BTC-EUR-100', 'order_type' => 0, 'quote_amount' => '60',
               'quote_amount_exec' => '60', 'amount_exec' => '0.00093', 'price' => '64000', 'created_at' => '2026-09-01 10:00:01' }
    waiting = lambda do |id, limit: false, created: '2026-09-01 10:00:01'|
      { 'status' => 0, 'external_status' => limit ? 1 : 0, 'external_id' => "BTC-EUR-#{id}", 'order_type' => limit ? 1 : 0,
        'quote_amount' => limit ? nil : '60', 'amount' => limit ? '0.00093' : nil, 'price' => '64000', 'amount_exec' => '0',
        'quote_amount_exec' => '0', 'created_at' => created }.compact
    end
    polled = ->(id, order) { { "GET /v2/order?market=BTC-EUR&orderId=#{id}" => [ok(order)] } }
    gone = ->(id) { { "GET /v2/order?market=BTC-EUR&orderId=#{id}" => [BV_NOT_FOUND] } }
    %w[day week].flat_map do |interval|
      step = { 'day' => 1.day, 'week' => 1.week }.fetch(interval)
      after = ->(n) { (Time.iso8601(started) + (n * step) + 1.second).iso8601(6) }
      variants = {
        'first_tick' => { 'at' => (Time.iso8601(started) + 0.5).iso8601(6) },
        'on_schedule' => { 'at' => after.(1), 'transactions' => [closed] },
        'carry' => { 'at' => after.(1), 'transient' => { 'missed_quote_amount' => '12.5' } },
        'below_minimum' => { 'at' => after.(1), 'quote_amount' => 2.0 },
        'base_sized' => { 'at' => after.(1), 'ticker' => ticker.merge('base_decimals' => 3, 'minimum_base_size' => '0.001', 'minimum_quote_size' => '1') },
        'quote_min_decides' => { 'at' => after.(1), 'ticker' => ticker.merge('minimum_quote_size' => '100') },
        'base_min_decides' => { 'at' => after.(1), 'ticker' => ticker.merge('base_decimals' => 2, 'minimum_base_size' => '0.01') },
        # 304 (outside the access window) is placement-safe: no row, the run rescheduled.
        'access_window_304' => { 'at' => after.(1), 'http' => { 'POST /v2/order' => [bv_err(403, 304, 'The request was not received within the access window.')] } },
        'insufficient' => { 'at' => after.(1), 'http' => { 'POST /v2/order' => [bv_err(400, 216, 'You do not have sufficient balance to complete this operation.')] } },
        'below_venue_minimum' => { 'at' => after.(1), 'http' => { 'POST /v2/order' => [bv_err(400, 217, 'Minimum order size in quote currency is 5 EUR.')] } },
        'invalid_key' => { 'at' => after.(1), 'transient' => { 'last_failure_kind' => 'invalid_key' },
                           'http' => { 'POST /v2/order' => [bv_err(403, 305, 'No active API key found.')] } },
        'bad_signature' => { 'at' => after.(1), 'http' => { 'POST /v2/order' => [bv_err(403, 309, 'The signature is invalid.')] } },
        'no_trade_permission' => { 'at' => after.(1), 'http' => { 'POST /v2/order' => [bv_err(403, 310, 'This key does not allow access to trade endpoints.')] } },
        'throttled' => { 'at' => after.(1), 'http' => { 'POST /v2/order' => [bv_err(429, 105, 'Your account has been temporarily banned for exceeding the rate limit.')] } },
        # Sanctioned divergences (rust/tests/parity.rs BITVAVO_DIVERGENCES): Rust keeps the intent, which waits for the operator
        # (plan VB §1.5), where Rails writes a failed row: a lost reply, a 5xx, an unreadable answer, a transient code (109
        # timed out: it may have succeeded), and an answer without an orderId (Rails stores "BTC-EUR-").
        'add_server_error' => { 'at' => after.(1), 'http' => { 'POST /v2/order' => [{ 'status' => 503, 'body' => '<html>upstream</html>' }] } },
        'add_timeout_after_send' => { 'at' => after.(1), 'http' => { 'POST /v2/order' => [POST_SEND] } },
        'add_no_order_id' => { 'at' => after.(1), 'http' => { 'POST /v2/order' => [ok('market' => 'BTC-EUR')] } },
        'add_unreadable' => { 'at' => after.(1), 'http' => { 'POST /v2/order' => [{ 'status' => 200, 'body' => '<html>upstream</html>' }] } },
        'add_timed_out_109' => { 'at' => after.(1), 'http' => { 'POST /v2/order' => [bv_err(400, 109, 'The request has timed out. Your order may have been placed.')] } },
        'add_network_pre_send' => { 'at' => after.(1), 'http' => { 'POST /v2/order' => [VENUES.fetch('bitvavo').not_sent] } },
        'low_funds' => { 'at' => after.(1), 'http' => { 'GET /v2/balance' => [bv_balance('1')] } },
        'balance_refused' => { 'at' => after.(1), 'http' => { 'GET /v2/balance' => [bv_err(403, 305, 'No active API key found.')] } },
        'zero_price' => { 'at' => after.(1), 'http' => { 'GET /v2/ticker/book' => [bv_book('0', bid: '0')], 'GET /v2/ticker/price' => [bv_last('0')] } },
        'no_price' => { 'at' => after.(1), 'http' => { 'GET /v2/ticker/book' => [bv_err(400, 205, 'market parameter is invalid.')],
                                                       'GET /v2/ticker/price' => [bv_err(400, 205, 'market parameter is invalid.')] } },
        # The tick's sweep (Bot::FetchAndUpdateOpenOrdersJob): #get_order per waiting order.
        'sweep_filled' => { 'at' => after.(1), 'transactions' => [waiting.(201)],
                            'http' => polled.(201, bv_order('201', 'filled', amount_quote: '60', filled: '0.00093', filled_quote: '59.5203')) },
        'sweep_new' => { 'at' => after.(1), 'transactions' => [waiting.(202, limit: true)],
                         'http' => polled.(202, bv_order('202', 'new', type: 'limit', price: '64000', amount: '0.00093')) },
        'sweep_partial' => { 'at' => after.(1), 'transactions' => [waiting.(203, limit: true)],
                             'http' => polled.(203, bv_order('203', 'partiallyFilled', type: 'limit', price: '64000', amount: '0.00093', filled: '0.0004', filled_quote: '25.5')) },
        'sweep_filled_limit_at_fills' => { 'at' => after.(1), 'transactions' => [waiting.(204, limit: true)],
                                           'http' => polled.(204, bv_order('204', 'filled', type: 'limit', price: '64000', amount: '0.00093', filled: '0.00093', filled_quote: '59.3')) },
        'sweep_cancelled_partial' => { 'at' => after.(1), 'transactions' => [waiting.(205, limit: true)],
                                       'http' => polled.(205, bv_order('205', 'canceled', type: 'limit', price: '64000', amount: '0.00093', filled: '0.0004', filled_quote: '25.6')) },
        'sweep_expired' => { 'at' => after.(1), 'transactions' => [waiting.(206)], 'http' => polled.(206, bv_order('206', 'expired', amount_quote: '60')) },
        'sweep_rejected' => { 'at' => after.(1), 'transactions' => [waiting.(207)], 'http' => polled.(207, bv_order('207', 'rejected', amount_quote: '60')) },
        'sweep_unknown_type' => { 'at' => after.(1), 'transactions' => [waiting.(208)], 'http' => polled.(208, bv_order('208', 'new', type: 'stopLoss')) },
        # A gone order (240): settled from its trades, or left to Bot::StaleOrderResolver (young waits, old is abandoned).
        'sweep_gone_filled' => { 'at' => after.(1), 'transactions' => [waiting.(209)],
                                 'http' => gone.(209).merge('GET /v2/trades' => [ok([bv_trade('t3', 'other', '1', '1', '2026-09-01T11:00:00Z'),
                                                                                     bv_trade('t2', '209', '0.0003', '64010', '2026-09-01T10:00:02Z'),
                                                                                     bv_trade('t1', '209', '0.0006', '64000', '2026-09-01T10:00:01Z')])]) },
        'sweep_gone_young' => { 'at' => after.(1), 'transactions' => [waiting.(210)], 'http' => gone.(210) },
        'sweep_gone_old' => { 'at' => after.(1), 'transactions' => [waiting.(211, created: '2026-08-01 10:00:01')], 'http' => gone.(211) },
        'sweep_not_found_html' => { 'at' => after.(1), 'transactions' => [waiting.(212)],
                                    'http' => { 'GET /v2/order?market=BTC-EUR&orderId=212' => [{ 'status' => 404, 'body' => '<html>Not Found</html>' }] } },
        # Poll-only: Bot::FetchAndUpdateOrderJob (#get_order) for one waiting order.
        'poll_filled' => { 'at' => after.(1), 'tick' => false, 'poll' => 'BTC-EUR-401', 'transactions' => [waiting.(401)],
                           'http' => polled.(401, bv_order('401', 'filled', amount_quote: '60', filled: '0.00093', filled_quote: '59.52')) },
        'poll_open' => { 'at' => after.(1), 'tick' => false, 'poll' => 'BTC-EUR-402', 'transactions' => [waiting.(402, limit: true)],
                         'http' => polled.(402, bv_order('402', 'new', type: 'limit', price: '64000', amount: '0.00093')) },
        'poll_gone_filled' => { 'at' => after.(1), 'tick' => false, 'poll' => 'BTC-EUR-403', 'transactions' => [waiting.(403)],
                                'http' => gone.(403).merge('GET /v2/trades' => [ok([bv_trade('t9', '403', '0.00093', '64000', '2026-09-01T10:00:01Z')])]) },
        'poll_gone_young' => { 'at' => after.(1), 'tick' => false, 'poll' => 'BTC-EUR-404', 'transactions' => [waiting.(404)], 'http' => gone.(404) },
        # A tick that places BV_ORDER; its follow-up 5 s later finds it filled.
        'tick_then_poll' => { 'at' => after.(1), 'poll' => "BTC-EUR-#{BV_ORDER}", 'transient' => { 'missed_quote_amount' => '12.5' },
                              'http' => polled.(BV_ORDER, bv_order(BV_ORDER, 'filled', amount_quote: '72.5', filled: '0.00112', filled_quote: '71.68')) }
      }
      modes.flat_map do |mode, settings|
        variants.map do |name, v|
          { 'name' => "#{interval}-#{mode}-#{name}", 'venue' => 'bitvavo', 'interval' => interval, 'quote_amount' => v.fetch('quote_amount', 60.0),
            'started_at' => started, 'settings' => settings, 'settings_changed_at' => v['settings_changed_at'],
            'transient' => v.fetch('transient', {}), 'transactions' => v.fetch('transactions', []), 'ticker' => v.fetch('ticker', ticker),
            'ticker_after' => v['ticker_after'], 'at' => v.fetch('at'), 'script' => { 'bitvavo' => http.merge(v.fetch('http', {})) },
            'tick' => v.fetch('tick', true), 'poll' => v['poll'] }
        end
      end
    end
  end

  # Gemini spot (plan VB B3), one asset on btcusd: Rails' own answers to the bodies below, beneath honeymaker. Every private
  # call is a POST (its params in the signed payload), named by its path.
  def gm_ticker(bid, ask, last) = ok('bid' => bid, 'ask' => ask, 'last' => last, 'volume' => { 'BTC' => '10', 'USD' => '640000', 'timestamp' => 1_788_000_000_000 })
  def gm_err(status, reason, message) = { 'status' => status, 'body' => { 'result' => 'error', 'reason' => reason, 'message' => message } }
  def gm_balances(usd, btc: '0.5') = ok([{ 'type' => 'exchange', 'currency' => 'BTC', 'amount' => btc, 'available' => btc, 'availableForWithdrawal' => btc },
                                       { 'type' => 'exchange', 'currency' => 'USD', 'amount' => usd, 'available' => usd, 'availableForWithdrawal' => usd }])
  def gm_order(id, live:, cancelled: false, type: 'exchange limit', side: 'buy', avg: '0', price: '64640.11', original: '0.00093749', executed: '0',
               options: ['immediate-or-cancel'])
    { 'order_id' => id.to_s, 'id' => id.to_s, 'symbol' => 'btcusd', 'exchange' => 'gemini', 'avg_execution_price' => avg, 'side' => side, 'type' => type,
      'timestamp' => '1788000000', 'timestampms' => 1_788_000_000_000, 'is_live' => live, 'is_cancelled' => cancelled, 'is_hidden' => false,
      'was_forced' => false, 'executed_amount' => executed, 'remaining_amount' => (BigDecimal(original) - BigDecimal(executed)).to_s('F'),
      'original_amount' => original, 'price' => price, 'options' => options }
  end
  GM_NOT_FOUND = { 'status' => 404, 'body' => { 'result' => 'error', 'reason' => 'OrderNotFound', 'message' => 'Order 1 not found' } }.freeze

  def gemini_scenarios
    started = '2026-09-01T10:00:00.123456Z'
    ticker = { 'base_decimals' => 8, 'quote_decimals' => 2, 'price_decimals' => 2, 'minimum_base_size' => '0.00001', 'minimum_quote_size' => '0' }
    http = {
      'GET /v1/pubticker/btcusd' => [gm_ticker('64000.01', '64000.11', '64000.05')],
      'POST /v1/order/new' => [ok(gm_order(7_612_345_678, live: false, executed: '0.00093749', avg: '64000.12'))],
      'POST /v1/balances' => [gm_balances('100000')]
    }
    modes = { 'market' => {}, 'limit' => { 'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.0025 } }
    closed = { 'status' => 0, 'external_status' => 2, 'external_id' => '100', 'order_type' => 0, 'quote_amount' => '60',
               'quote_amount_exec' => '60', 'amount_exec' => '0.00093', 'price' => '64000', 'created_at' => '2026-09-01 10:00:01' }
    waiting = lambda do |id, limit: false, created: '2026-09-01 10:00:01'|
      { 'status' => 0, 'external_status' => limit ? 1 : 0, 'external_id' => id.to_s, 'order_type' => limit ? 1 : 0,
        'quote_amount' => limit ? nil : '60', 'amount' => limit ? '0.00093' : nil, 'price' => '64000', 'amount_exec' => '0',
        'quote_amount_exec' => '0', 'created_at' => created }.compact
    end
    polled = ->(order) { { 'POST /v1/order/status' => [ok(order)] } }
    %w[day week].flat_map do |interval|
      step = { 'day' => 1.day, 'week' => 1.week }.fetch(interval)
      after = ->(n) { (Time.iso8601(started) + (n * step) + 1.second).iso8601(6) }
      variants = {
        'first_tick' => { 'at' => (Time.iso8601(started) + 0.5).iso8601(6) },
        'on_schedule' => { 'at' => after.(1), 'transactions' => [closed] },
        'carry' => { 'at' => after.(1), 'transient' => { 'missed_quote_amount' => '12.5' } },
        'below_minimum' => { 'at' => after.(1), 'quote_amount' => 0.5 },
        # D-679: 60 at the raw ask is 0.00093749 base (over the minimum), at the crossed ask 0.00092821 (under it).
        'cross_below_minimum' => { 'at' => (Time.iso8601(started) + 0.5).iso8601(6), 'ticker' => ticker.merge('minimum_base_size' => '0.00093') },
        'coarse_base' => { 'at' => after.(1), 'ticker' => ticker.merge('base_decimals' => 5, 'minimum_base_size' => '0.0001') },
        'coarse_price' => { 'at' => after.(1), 'ticker' => ticker.merge('price_decimals' => 0, 'quote_decimals' => 0),
                            'http' => { 'GET /v1/pubticker/btcusd' => [gm_ticker('64000.9', '64001.7', '64001.3')] } },
        # InvalidNonce is placement-safe (M2): no row, the run rescheduled.
        'invalid_nonce' => { 'at' => after.(1), 'http' => { 'POST /v1/order/new' => [gm_err(400, 'InvalidNonce', 'Nonce 1 has not increased since your last call')] } },
        'insufficient' => { 'at' => after.(1), 'http' => { 'POST /v1/order/new' => [gm_err(400, 'InsufficientFunds', 'Failed to place buy order: insufficient funds')] } },
        'invalid_key' => { 'at' => after.(1), 'transient' => { 'last_failure_kind' => 'invalid_key' },
                           'http' => { 'POST /v1/order/new' => [gm_err(400, 'InvalidSignature', 'InvalidSignature')] } },
        'missing_role' => { 'at' => after.(1), 'http' => { 'POST /v1/order/new' => [gm_err(403, 'MissingRole', 'API key lacks the required role')] } },
        'rate_limited' => { 'at' => after.(1), 'http' => { 'POST /v1/order/new' => [gm_err(429, 'RateLimit', 'Requests were made too frequently')] } },
        'invalid_price' => { 'at' => after.(1), 'http' => { 'POST /v1/order/new' => [gm_err(400, 'InvalidPrice', 'Invalid price for symbol BTCUSD')] } },
        # Sanctioned divergences (rust/tests/parity.rs GEMINI_DIVERGENCES): Rust keeps the intent, which on Gemini a client order
        # id lookup may find; Rails writes a failed row.
        'add_server_error' => { 'at' => after.(1), 'http' => { 'POST /v1/order/new' => [gm_err(500, 'System', 'We are experiencing technical issues')] } },
        'add_maintenance' => { 'at' => after.(1), 'http' => { 'POST /v1/order/new' => [gm_err(503, 'Maintenance', 'The system is down for maintenance')] } },
        'add_timeout_after_send' => { 'at' => after.(1), 'http' => { 'POST /v1/order/new' => [POST_SEND] } },
        'add_no_order_id' => { 'at' => after.(1), 'http' => { 'POST /v1/order/new' => [ok('symbol' => 'btcusd')] } },
        'add_unreadable' => { 'at' => after.(1), 'http' => { 'POST /v1/order/new' => [{ 'status' => 200, 'body' => '<html>upstream</html>' }] } },
        # Landed recovery (Rust only, `recover_at`): the client order id lookup finds the order, or finds nothing.
        'landed_found' => { 'at' => after.(1), 'recover_at' => (Time.iso8601(after.(1)) + 60).iso8601(6),
                            'http' => { 'POST /v1/order/new' => [POST_SEND],
                                        'POST /v1/order/status' => [ok(gm_order(7_612_345_679, live: false, executed: '0.00092821', original: '0.00092821', avg: '64000.12')
                                                                         .merge('client_order_id' => '$client_order_id'))] } },
        'landed_missing' => { 'at' => after.(1), 'recover_at' => (Time.iso8601(after.(1)) + 60).iso8601(6),
                              'http' => { 'POST /v1/order/new' => [POST_SEND], 'POST /v1/order/status' => [GM_NOT_FOUND] } },
        'add_network_pre_send' => { 'at' => after.(1), 'http' => { 'POST /v1/order/new' => [VENUES.fetch('gemini').not_sent] } },
        'low_funds' => { 'at' => after.(1), 'http' => { 'POST /v1/balances' => [gm_balances('1')] } },
        'balance_refused' => { 'at' => after.(1), 'http' => { 'POST /v1/balances' => [gm_err(400, 'InvalidApiKey', 'Invalid API key')] } },
        'zero_price' => { 'at' => after.(1), 'http' => { 'GET /v1/pubticker/btcusd' => [gm_ticker('0', '0', '0')] } },
        'no_price' => { 'at' => after.(1), 'http' => { 'GET /v1/pubticker/btcusd' => [gm_err(400, 'InvalidSymbol', 'Invalid symbol')] } },
        # The tick's sweep (Bot::FetchAndUpdateOpenOrdersJob): #get_order per waiting order.
        'sweep_closed' => { 'at' => after.(1), 'transactions' => [waiting.(201)],
                            'http' => polled.(gm_order(201, live: false, executed: '0.00093749', avg: '64000.12')) },
        'sweep_ioc_partial' => { 'at' => after.(1), 'transactions' => [waiting.(202)],
                                 'http' => polled.(gm_order(202, live: false, cancelled: true, executed: '0.0004', avg: '64000.5')) },
        'sweep_ioc_unfilled' => { 'at' => after.(1), 'transactions' => [waiting.(203)], 'http' => polled.(gm_order(203, live: false, cancelled: true)) },
        'sweep_open_limit' => { 'at' => after.(1), 'transactions' => [waiting.(204, limit: true)],
                                'http' => polled.(gm_order(204, live: true, price: '63840.04', original: '0.00093', options: [])) },
        'sweep_limit_partial' => { 'at' => after.(1), 'transactions' => [waiting.(205, limit: true)],
                                   'http' => polled.(gm_order(205, live: true, price: '63840.04', original: '0.00093', executed: '0.0004', avg: '63840.04', options: [])) },
        'sweep_unknown' => { 'at' => after.(1), 'transactions' => [waiting.(206)], 'http' => polled.(gm_order(206, live: false)) },
        'sweep_market_type' => { 'at' => after.(1), 'transactions' => [waiting.(207)],
                                 'http' => polled.(gm_order(207, live: false, type: 'exchange market', executed: '0.00093', avg: '64000')) },
        'sweep_not_found_young' => { 'at' => after.(1), 'transactions' => [waiting.(208)], 'http' => { 'POST /v1/order/status' => [GM_NOT_FOUND] } },
        'sweep_not_found_old' => { 'at' => after.(1), 'transactions' => [waiting.(209, created: '2026-08-01 10:00:01')],
                                   'http' => { 'POST /v1/order/status' => [GM_NOT_FOUND] } },
        'sweep_failed' => { 'at' => after.(1), 'transactions' => [waiting.(210)], 'http' => { 'POST /v1/order/status' => [gm_err(500, 'System', 'Internal error')] } },
        # Poll-only: Bot::FetchAndUpdateOrderJob (#get_order) for one waiting order.
        'poll_filled' => { 'at' => after.(1), 'tick' => false, 'poll' => '401', 'transactions' => [waiting.(401)],
                           'http' => polled.(gm_order(401, live: false, executed: '0.00093749', avg: '64000.12')) },
        'poll_open' => { 'at' => after.(1), 'tick' => false, 'poll' => '402', 'transactions' => [waiting.(402, limit: true)],
                         'http' => polled.(gm_order(402, live: true, price: '63840.04', original: '0.00093', options: [])) },
        'poll_not_found' => { 'at' => after.(1), 'tick' => false, 'poll' => '403', 'transactions' => [waiting.(403)], 'http' => { 'POST /v1/order/status' => [GM_NOT_FOUND] } },
        # A tick that places 7612345678; its follow-up 5 s later finds it filled.
        'tick_then_poll' => { 'at' => after.(1), 'poll' => '7612345678', 'transient' => { 'missed_quote_amount' => '12.5' },
                              'http' => polled.(gm_order(7_612_345_678, live: false, executed: '0.00112', original: '0.00112', avg: '64000.12')) }
      }
      modes.flat_map do |mode, settings|
        variants.map do |name, v|
          { 'name' => "#{interval}-#{mode}-#{name}", 'venue' => 'gemini', 'interval' => interval, 'quote_amount' => v.fetch('quote_amount', 60.0),
            'started_at' => started, 'settings' => settings, 'settings_changed_at' => v['settings_changed_at'],
            'transient' => v.fetch('transient', {}), 'transactions' => v.fetch('transactions', []), 'ticker' => v.fetch('ticker', ticker),
            'ticker_after' => v['ticker_after'], 'at' => v.fetch('at'), 'recover_at' => v['recover_at'], 'script' => { 'gemini' => http.merge(v.fetch('http', {})) },
            'tick' => v.fetch('tick', true), 'poll' => v['poll'] }.compact
        end
      end
    end
  end

  # Slice 7a: the price rule of the active side with the `pause` action, on all three venues and both directions. The tick at
  # `at` passes, pauses or fails; a scenario with `check_at` then runs Bot::PriceLimitCheckJob there (and the Bot::ActionJob it
  # enqueues when met). Every check job a phase queues is recorded with its time.
  def condition_scenarios
    started = '2026-09-01T10:00:00.123456Z'
    at = (Time.iso8601(started) + 1.day + 1.second).iso8601(6)
    check_at = (Time.iso8601(at) + 180).iso8601(6)
    met_before = '2026-08-31T10:00:00.000Z'
    kr_balance = { 'error' => [], 'result' => { 'ZEUR' => { 'balance' => '100000', 'hold_trade' => '0' }, 'XXBT' => { 'balance' => '0.6', 'hold_trade' => '0.1' } } }
    al_fail = ->(status, code, message) { { 'status' => status, 'body' => { 'code' => code, 'message' => message } } }
    al_book = ok('quotes' => { 'BTC/USD' => { 'ap' => 64_321.479, 'as' => 0.5, 'bp' => 64_300.25, 'bs' => 0.4, 't' => '2026-09-01T10:00:00Z' } })
    venues = {
      'kraken' => {
        'price' => BigDecimal('49995.3'),
        'ticker' => { 'base_decimals' => 8, 'quote_decimals' => 5, 'price_decimals' => 1, 'minimum_base_size' => '0.00005', 'minimum_quote_size' => '0.5' },
        'http' => { '/0/private/AddOrder' => [{ 'error' => [], 'result' => { 'txid' => ['OTX-1'] } }], '/0/private/BalanceEx' => [kr_balance],
                    '/0/private/QueryOrders' => [query_body({})], '/0/private/TradesHistory' => [{ 'error' => [], 'result' => { 'trades' => {}, 'count' => 0 } }] },
        'last' => ->(prices) { { '/0/public/Ticker' => prices.map { |l| l.is_a?(Hash) ? l : ticker_body('49990.1', '50000.2', l.to_s('F')) } } },
        'server_error' => { 'error' => ['EService:Unavailable'] },
        'rejected_key' => { 'error' => ['EAPI:Invalid key'] }
      },
      'alpaca' => {
        'price' => BigDecimal('64310.75'),
        'ticker' => { 'base_decimals' => 9, 'quote_decimals' => 2, 'price_decimals' => 2, 'minimum_base_size' => '0.000027', 'minimum_quote_size' => '1' },
        'http' => { 'GET /v1beta3/crypto/us/latest/quotes' => [al_book], 'POST /v2/orders' => [ok(alpaca_order('OTX-1', 'pending_new'))],
                    'GET /v2/account' => [account('100000')], 'GET /v2/positions' => [ok([position('BTCUSD', '0.6').merge('qty_available' => '0.5')])],
                    'GET /v2/clock' => [clock(true)] },
        'last' => ->(prices) { { 'GET /v1beta3/crypto/us/latest/trades' => prices.map { |l| l.is_a?(Hash) ? l : trades(l.to_f) } } },
        'server_error' => al_fail.(500, 50_010_000, 'internal server error'),
        'rejected_key' => al_fail.(401, 40_110_000, 'unauthorized.')
      },
      'binance' => {
        'price' => BigDecimal('64000.05'),
        'ticker' => { 'base_decimals' => 5, 'quote_decimals' => 8, 'price_decimals' => 2, 'minimum_base_size' => '0.00001', 'minimum_quote_size' => '5' },
        'http' => { 'GET /api/v3/ticker/bookTicker' => [bn_book('64000.11000000')],
                    'POST /api/v3/order' => [ok('symbol' => 'BTCUSDT', 'orderId' => 28_457, 'orderListId' => -1, 'clientOrderId' => 'x', 'transactTime' => 1_788_000_000_000)],
                    'GET /api/v3/account' => [ok('makerCommission' => 10, 'canTrade' => true, 'balances' => [{ 'asset' => 'BTC', 'free' => '0.5', 'locked' => '0.1' },
                                                                                                        { 'asset' => 'USDT', 'free' => '100000.00000000', 'locked' => '0' }])],
                    'GET /api/v3/allOrders' => [ok([])], 'GET /api/v3/myTrades' => [ok([])] },
        'last' => ->(prices) { { 'GET /api/v3/ticker/price' => prices.map { |l| l.is_a?(Hash) ? l : bn_last(l.to_s('F')) } } },
        'server_error' => bn_fail(500, -1001, 'Internal error; unable to process your request. Please try your request again.'),
        'rejected_key' => bn_fail(401, -2015, 'Invalid API-key, IP, or permissions for action.')
      }
    }
    sides = { 'buy' => ['price_limit', {}, { 'missed_quote_amount' => '30' }],
              'sell' => ['sell_price_limit', { 'direction' => 'selling', 'sell_interval' => 'day', 'sell_denomination' => 'base', 'sell_amount' => 0.01 },
                         { 'missed_quote_amount' => '37.5' }] }
    venues.flat_map do |venue, vn|
      p = vn['price']
      hi = (p + 100).to_f
      lo = (p - 100).to_f
      exact = p.to_f
      unmet_then = ->(*later) { [p] + later }
      cases = {
        'below_met' => { 'limit' => hi }, 'below_unmet' => { 'limit' => lo }, 'below_at_limit' => { 'limit' => exact },
        'below_integer_limit' => { 'limit' => 1_000_000 }, 'below_default_limit' => { 'limit' => nil },
        'above_met' => { 'value' => 'above', 'limit' => lo }, 'above_unmet' => { 'value' => 'above', 'limit' => hi },
        'above_at_limit' => { 'value' => 'above', 'limit' => exact },
        'between_reversed_met' => { 'value' => 'between', 'lower' => hi, 'upper' => lo },
        'between_unmet' => { 'value' => 'between', 'lower' => hi, 'upper' => hi + 100 },
        'between_at_bound' => { 'value' => 'between', 'lower' => exact, 'upper' => hi },
        'after_stored' => { 'timing' => 'after', 'limit' => lo, 'met_at' => met_before },
        'after_unmet' => { 'timing' => 'after', 'limit' => lo }, 'after_met' => { 'timing' => 'after', 'limit' => hi },
        'while_stored_met' => { 'limit' => hi, 'met_at' => met_before },
        'while_stored_unmet' => { 'limit' => lo, 'met_at' => met_before },
        'missing_ticker' => { 'limit' => hi, 'met_at' => met_before, 'ticker_after' => { 'available' => false } },
        'missing_ticker_unset' => { 'limit' => hi, 'ticker_after' => { 'available' => false } },
        'zero_price' => { 'limit' => hi, 'met_at' => met_before, 'last' => [BigDecimal('0')] },
        'server_error' => { 'limit' => hi, 'met_at' => met_before, 'last' => [vn['server_error']] },
        'rejected_key' => { 'limit' => hi, 'last' => [vn['rejected_key']] },
        'other_side_only' => { 'off' => true },
        'check_met' => { 'limit' => p.to_f - 50, 'last' => unmet_then.(p - 60), 'check' => true },
        'check_met_no_carry' => { 'limit' => p.to_f - 50, 'last' => unmet_then.(p - 60), 'check' => true, 'transient' => { 'missed_quote_amount' => nil } },
        'check_unmet' => { 'limit' => lo, 'last' => unmet_then.(p + 1), 'check' => true },
        'check_lost_then_met' => { 'limit' => p.to_f - 50, 'met_at' => met_before, 'last' => unmet_then.(p - 60), 'check' => true },
        'check_server_error' => { 'limit' => lo, 'last' => unmet_then.(vn['server_error']), 'check' => true },
        'check_zero' => { 'limit' => lo, 'last' => unmet_then.(BigDecimal('0')), 'check' => true },
        'check_rejected_key' => { 'limit' => lo, 'last' => unmet_then.(vn['rejected_key']), 'check' => true }
      }
      sides.flat_map do |side, (prefix, direction, transient)|
        other = side == 'buy' ? 'sell_price_limit' : 'price_limit'
        cases.map do |name, c|
          rule = if c['off']
                   { "#{other}ed" => true, other => 0.0, "#{other}_value_condition" => 'below' } # the inactive side's rule only
                 else
                   { "#{prefix}ed" => true, "#{prefix}_value_condition" => c.fetch('value', 'below'),
                     "#{prefix}_timing_condition" => c.fetch('timing', 'while'), "#{prefix}_action" => 'pause' }
                     .merge(c.key?('limit') ? { prefix => c['limit'] } : {})
                     .merge(c.key?('lower') ? { "#{prefix}_range_lower_bound" => c['lower'], "#{prefix}_range_upper_bound" => c['upper'] } : {})
                 end
          rule = rule.compact
          { 'name' => "cond-#{venue}-#{side}-#{name}", 'venue' => (venue unless venue == 'kraken'), 'interval' => 'week', 'quote_amount' => 60.0,
            'started_at' => started, 'settings' => direction.merge(rule),
            'transient' => transient.merge(c['met_at'] ? { "#{prefix}_condition_met_at" => c['met_at'] } : {}).merge(c.fetch('transient', {})).compact,
            'transactions' => [], 'ticker' => vn['ticker'], 'ticker_after' => c['ticker_after'], 'at' => at,
            'check_at' => (check_at if c['check']), 'conditions' => true,
            'script' => { (venue == 'kraken' ? 'http' : venue) => vn['http'].merge(vn['last'].(c.fetch('last', [p]))) }, 'tick' => true }
        end
      end
    end
  end

  # ---- Plan 8a: Bot::SignalJob, one webhook call of a signal bot (grid-signal) --------------------------------------------
  # A weekday session (10:00 EDT), so the stock clock is open.
  SIGNAL_AT = '2026-09-01T14:00:01.250000Z'
  SIGNAL_STARTED = '2026-08-25T09:00:00.123456Z'

  # A Bots::Signal as BotApi::Bots::CreateSignal leaves it (created, started), with its rules, on one of the four venues.
  def build_signal(dir, sc)
    SchemaTemplate.install(dir)
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
    user = User.new(name: 'Owner', email: 'owner@example.com', encrypted_password: SchemaTemplate.owner_password('correct horse battery staple'), admin: true,
                    confirmed_at: Time.current, setup_completed: true)
    user.save!(validate: false)
    exchange, base, quote, _ticker, passphrase = case sc['venue']
                                                 when 'alpaca' then sc['asset'] == 'stock' ? signal_stock_venue : alpaca_venue(sc)
                                                 when *VENUES.keys then venue_row(VENUES.fetch(sc['venue']), sc)
                                                 else kraken_venue(sc)
                                                 end
    unless sc['no_key']
      ApiKey.new(user:, exchange:, key: 'k', secret: 's', passphrase:, status: sc.fetch('key_status', 'correct'), key_type: :trading).save!(validate: false)
    end
    bot = user.bots.new(type: 'Bots::Signal', exchange:, settings: { 'base_asset_id' => base.id, 'quote_asset_id' => quote.id })
    bot.save!
    bot.update_columns(status: Bot.statuses.fetch(sc.fetch('status', 'scheduled')), started_at: Time.iso8601(SIGNAL_STARTED),
                       transient_data: bot.reload.transient_data.merge(sc.fetch('transient', {})))
    sc['rules'].each { |r| BotSignal.create!(bot:, direction: r['direction'], amount_type: r['amount_type'], amount: r['amount'], enabled: r.fetch('enabled', true)) }
    sc.fetch('transactions', []).each do |t|
      Transaction.insert!(t.merge('bot_id' => bot.id, 'exchange_id' => exchange.id, 'base_asset_id' => base.id, 'quote_asset_id' => quote.id,
                                  'base' => base.symbol, 'quote' => quote.symbol, 'side' => t.fetch('side', 0), 'transaction_type' => 'REGULAR',
                                  'bot_interval' => '', 'bot_quote_amount' => 0, 'error_messages' => t.fetch('error_messages', []), 'updated_at' => t['created_at']))
    end
    bot
  end

  def signal_stock_venue
    alpaca = Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25')
    usd = Asset.create!(external_id: 'usd', symbol: 'USD', name: 'US Dollar', category: 'Fiat')
    aapl = Asset.create!(external_id: 'AAPL.US', symbol: 'AAPL', name: 'AAPL', category: 'Stock', instrument_type: 'stock')
    [usd, aapl].each { |a| ExchangeAsset.create!(exchange: alpaca, asset: a, available: true) }
    ticker = Ticker.create!(exchange: alpaca, ticker: 'AAPL', base: 'AAPL', quote: 'USD', base_asset: aapl, quote_asset: usd,
                            **MarketData::STOCK_TICKER_DEFAULTS.transform_keys(&:to_sym))
    [alpaca, aapl, usd, ticker, 'paper']
  end

  def sg_kraken_order(side, vol, cost, price) = { 'status' => 'closed', 'vol' => vol, 'vol_exec' => vol, 'cost' => cost, 'price' => price, 'oflags' => side == 'buy' ? 'viqc' : '',
                                                    'descr' => { 'pair' => 'XBTEUR', 'type' => side, 'ordertype' => 'market', 'price' => '0' } }
  def sg_alpaca_fail(status, code, message) = { 'status' => status, 'body' => { 'code' => code, 'message' => message } }

  # Per venue: the ticker, the bodies a call that places reads, and the replies each case swaps in. Prices: bid / ask.
  def signal_venues
    kr_balance = ->(eur, xbt) { ok('error' => [], 'result' => { 'ZEUR' => { 'balance' => eur, 'hold_trade' => '0' }, 'XXBT' => { 'balance' => xbt, 'hold_trade' => '0' } }) }
    kr_net = KRAKEN_NOT_SENT
    bn_net = VENUES.fetch('binance').not_sent
    kc_net = VENUES.fetch('kucoin').not_sent
    bn_holding = ->(usdt, btc) { ok('makerCommission' => 10, 'canTrade' => true, 'balances' => [{ 'asset' => 'BTC', 'free' => btc, 'locked' => '0' }, { 'asset' => 'USDT', 'free' => usdt, 'locked' => '0' }]) }
    kc_holding = ->(usdt, btc) { kc_ok([{ 'id' => 'a1', 'currency' => 'BTC', 'type' => 'trade', 'balance' => btc, 'available' => btc, 'holds' => '0' },
                                        { 'id' => 'a2', 'currency' => 'USDT', 'type' => 'trade', 'balance' => usdt, 'available' => usdt, 'holds' => '0' }]) }
    al_book = ->(bid, ask) { ok('quotes' => { 'BTC/USD' => { 'ap' => ask, 'as' => 0.5, 'bp' => bid, 'bs' => 0.4, 't' => '2026-09-01T14:00:00Z' } }) }
    al_held = ->(sym, cls, qty) { ok(qty.nil? ? [] : [{ 'symbol' => sym, 'asset_class' => cls, 'qty' => qty, 'qty_available' => qty, 'side' => 'long' }]) }
    stock_book = ->(bid, ask) { ok('symbol' => 'AAPL', 'quote' => { 'ap' => ask, 'as' => 100, 'bp' => bid, 'bs' => 100, 't' => '2026-09-01T14:00:00Z' }) }
    {
      'kraken' => {
        'ticker' => { 'base_decimals' => 8, 'quote_decimals' => 5, 'price_decimals' => 1, 'minimum_base_size' => '0.00005', 'minimum_quote_size' => '0.5' },
        'key' => 'kraken_http', 'order' => 'OTX-1', 'second' => ok('error' => [], 'result' => { 'txid' => ['OTX-2'] }),
        'http' => { 'GET /0/public/Ticker?pair=XBTEUR' => [ok(kr_ticker('XBTEUR', '49990.1', '50000.2', '49995.3'))],
                    'POST /0/private/BalanceEx' => [kr_balance.('10000', '0.5')],
                    'POST /0/private/AddOrder' => [ok('error' => [], 'result' => { 'txid' => ['OTX-1'] })],
                    'POST /0/private/TradesHistory' => [ok('error' => [], 'result' => { 'trades' => {}, 'count' => 0 })] },
        'filled' => ->(side) { { 'POST /0/private/QueryOrders' => [ok(query_body('OTX-1' => sg_kraken_order(side, '0.002', '100.0', '50000.2')))] } },
        'free' => ->(btc) { { 'POST /0/private/BalanceEx' => [kr_balance.('10000', btc)] } },
        'price_transient' => [kr_net], 'price_refused' => [ok('error' => ['EQuery:Unknown asset pair'])], 'price_key' => 'GET /0/public/Ticker?pair=XBTEUR',
        'balance_failed' => { 'POST /0/private/BalanceEx' => [ok('error' => ['EAPI:Invalid key'])] },
        'add' => 'POST /0/private/AddOrder', 'rejected' => ok('error' => ['EOrder:Insufficient funds']), 'safe' => nil,
        'server_error' => { 'status' => 503, 'body' => { 'error' => ['EService:Unavailable'] } }, 'no_order_id' => ok('error' => [], 'result' => {}),
        'pre_send' => kr_net
      },
      'binance' => {
        'ticker' => { 'base_decimals' => 5, 'quote_decimals' => 8, 'price_decimals' => 2, 'minimum_base_size' => '0.00001', 'minimum_quote_size' => '5' },
        'key' => 'binance', 'order' => 'BTCUSDT-28457',
        'second' => ok('symbol' => 'BTCUSDT', 'orderId' => 28_458, 'orderListId' => -1, 'clientOrderId' => 'y', 'transactTime' => 1_788_000_000_000),
        'http' => { 'GET /api/v3/ticker/bookTicker' => [bn_book('64000.11000000')], 'GET /api/v3/account' => [bn_holding.('10000.00000000', '0.50000000')],
                    'POST /api/v3/order' => [ok('symbol' => 'BTCUSDT', 'orderId' => 28_457, 'orderListId' => -1, 'clientOrderId' => 'x', 'transactTime' => 1_788_000_000_000)],
                    'GET /api/v3/myTrades' => [ok([])] },
        'filled' => ->(side) { { 'GET /api/v3/order' => [ok(bn_order(28_457, 'FILLED', executed: '0.00156000', cumulative: '99.84000000').merge('side' => side.upcase))] } },
        'free' => ->(btc) { { 'GET /api/v3/account' => [bn_holding.('10000.00000000', btc)] } },
        'price_transient' => [bn_net], 'price_refused' => [bn_fail(400, -1121, 'Invalid symbol.')], 'price_key' => 'GET /api/v3/ticker/bookTicker',
        'balance_failed' => { 'GET /api/v3/account' => [bn_fail(400, -2015, 'Invalid API-key, IP, or permissions for action.')] },
        'add' => 'POST /api/v3/order', 'rejected' => bn_fail(400, -2010, 'Account has insufficient balance for requested action.'),
        'safe' => bn_fail(400, -1021, BN_1021),
        'server_error' => bn_fail(503, -1001, 'Internal error; unable to process your request. Please try again.'), 'no_order_id' => ok('symbol' => 'BTCUSDT'),
        'pre_send' => bn_net
      },
      'kucoin' => {
        'ticker' => { 'base_decimals' => 8, 'quote_decimals' => 6, 'price_decimals' => 1, 'minimum_base_size' => '0.00001', 'minimum_quote_size' => '0.1' },
        'key' => 'kucoin', 'order' => '6716f0a2c1b6d10007a3b1c5', 'second' => kc_ok('orderId' => '6716f0a2c1b6d10007a3b1c6'),
        'http' => { 'GET /api/v1/market/orderbook/level2_20' => [kc_book('64000.2')], 'GET /api/v1/accounts' => [kc_holding.('10000', '0.5')],
                    'POST /api/v1/orders' => [kc_ok('orderId' => '6716f0a2c1b6d10007a3b1c5')], 'GET /api/v1/fills' => [kc_fills([])] },
        'filled' => ->(side) { { 'GET /api/v1/orders/6716f0a2c1b6d10007a3b1c5' => [kc_ok(kc_order('6716f0a2c1b6d10007a3b1c5', active: false, deal_size: '0.00156', deal_funds: '99.84').merge('side' => side))] } },
        'free' => ->(btc) { { 'GET /api/v1/accounts' => [kc_holding.('10000', btc)] } },
        'price_transient' => [kc_net], 'price_refused' => [ok('code' => '400100', 'msg' => 'symbol not exists')], 'price_key' => 'GET /api/v1/market/orderbook/level2_20',
        'balance_failed' => { 'GET /api/v1/accounts' => [ok('code' => '400003', 'msg' => 'KC-API-KEY not exists')] },
        'add' => 'POST /api/v1/orders', 'rejected' => ok('code' => '200004', 'msg' => 'Balance insufficient!'),
        'safe' => ok('code' => '400002', 'msg' => 'Invalid KC-API-TIMESTAMP'),
        'server_error' => { 'status' => 503, 'body' => '<html>upstream</html>' }, 'no_order_id' => kc_ok({}),
        'pre_send' => kc_net
      },
      'alpaca' => {
        'ticker' => { 'base_decimals' => 9, 'quote_decimals' => 2, 'price_decimals' => 2, 'minimum_base_size' => '0.000027', 'minimum_quote_size' => '1' },
        'key' => 'alpaca', 'order' => 'OTX-1', 'second' => ok(alpaca_order('OTX-2', 'pending_new')),
        'http' => { 'GET /v1beta3/crypto/us/latest/quotes' => [al_book.(64_300.25, 64_321.47)], 'GET /v2/account' => [account('10000', '4000')],
                    'GET /v2/positions' => [al_held.('BTCUSD', 'crypto', '0.5')], 'POST /v2/orders' => [ok(alpaca_order('OTX-1', 'pending_new'))] },
        'filled' => ->(side) { { 'GET /v2/orders/OTX-1' => [ok(alpaca_order('OTX-1', 'filled', filled_qty: '0.00155', filled_avg_price: '64321.47').merge('side' => side))] } },
        'free' => ->(btc) { { 'GET /v2/positions' => [al_held.('BTCUSD', 'crypto', btc == '0' ? nil : btc)] } },
        'price_transient' => [DATA_PRE_SEND], 'price_refused' => [sg_alpaca_fail(403, 40_310_000, 'forbidden.')], 'price_key' => 'GET /v1beta3/crypto/us/latest/quotes',
        'balance_failed' => { 'GET /v2/account' => [sg_alpaca_fail(403, 40_310_000, 'forbidden.')], 'GET /v2/positions' => [sg_alpaca_fail(403, 40_310_000, 'forbidden.')] },
        'add' => 'POST /v2/orders', 'rejected' => sg_alpaca_fail(403, 40_310_000, 'insufficient balance for USD (requested: 100, available: 50)'), 'safe' => nil,
        'server_error' => sg_alpaca_fail(500, 50_010_000, 'internal server error'), 'no_order_id' => ok('status' => 'accepted'),
        'pre_send' => PRE_SEND
      },
      'alpaca_stock' => {
        'venue' => 'alpaca', 'asset' => 'stock', 'key' => 'alpaca', 'order' => 'OTX-1', 'second' => ok(stock_order('OTX-2', 'new')),
        'http' => { 'GET /v2/clock' => [open_clock(SIGNAL_AT)], 'GET /v2/stocks/AAPL/quotes/latest' => [stock_book.(187.41, 187.43)],
                    'GET /v2/account' => [stock_account('10000', '20000')], 'GET /v2/positions' => [al_held.('AAPL', 'us_equity', '3')],
                    'POST /v2/orders' => [ok(stock_order('OTX-1', 'new'))] },
        'filled' => ->(side) { { 'GET /v2/orders/OTX-1' => [ok(stock_order('OTX-1', 'filled', filled_qty: '0.533', filled_avg_price: '187.43').merge('side' => side))] } },
        'free' => ->(qty) { { 'GET /v2/positions' => [al_held.('AAPL', 'us_equity', qty == '0' ? nil : qty)] } },
        'price_transient' => [DATA_PRE_SEND], 'price_refused' => [sg_alpaca_fail(403, 40_310_000, 'forbidden.')], 'price_key' => 'GET /v2/stocks/AAPL/quotes/latest',
        'balance_failed' => { 'GET /v2/account' => [sg_alpaca_fail(403, 40_310_000, 'forbidden.')], 'GET /v2/positions' => [sg_alpaca_fail(403, 40_310_000, 'forbidden.')] },
        'add' => 'POST /v2/orders', 'rejected' => sg_alpaca_fail(403, 40_310_000, 'insufficient buying power'), 'safe' => nil,
        'server_error' => sg_alpaca_fail(500, 50_010_000, 'internal server error'), 'no_order_id' => ok('status' => 'accepted'),
        'pre_send' => PRE_SEND
      }
    }
  end

  def signal_scenarios
    at = Time.iso8601(SIGNAL_AT)
    before = ->(s) { (at - s).iso8601(6) }
    row = ->(status, ext, created) { { 'status' => status, 'external_status' => ext, 'external_id' => ext && "PRIOR-#{created.delete('^0-9')}", 'order_type' => 0,
                                       'amount' => '0.001', 'quote_amount' => '50', 'price' => '50000', 'amount_exec' => '0', 'quote_amount_exec' => '0', 'created_at' => created } }
    failed = row.(1, nil, '2026-09-01 12:00:00').merge('error_messages' => ['an earlier failure'])
    skipped = row.(2, nil, '2026-09-01 13:00:00')
    submitted = row.(0, 2, '2026-09-01 13:00:00')
    marker = { 'signal_placement_unresolved' => { 'id' => 'marker-1', 'at' => '2026-09-01T13:59:00Z', 'side' => 'buy', 'error' => 'Faraday::TimeoutError: Net::ReadTimeout' } }
    rules = { 'buy-fixed' => { 'direction' => 'buy', 'amount_type' => 'fixed', 'amount' => '100' },
              'buy-percentage' => { 'direction' => 'buy', 'amount_type' => 'percentage', 'amount' => '10' },
              'sell-fixed' => { 'direction' => 'sell', 'amount_type' => 'fixed', 'amount' => '100' },
              'sell-percentage' => { 'direction' => 'sell', 'amount_type' => 'percentage', 'amount' => '50' } }
    list = []
    signal_venues.each do |vkey, vn|
      add = vn['add']
      # The decision before any sizing: one rule is enough (the rule's amount is never read).
      gates = { 'expired' => { 'triggered_at' => before.(300.000001) }, 'at_max_age' => { 'triggered_at' => before.(300), 'filled' => true },
                'stopped' => { 'status' => 'stopped' }, 'disabled' => { 'enabled' => false }, 'key_pending' => { 'key_status' => 'pending_activation' },
                # No key: not here. The install refuses a working bot without one (alpaca::preflight), so no call runs.
                'unresolved_marker' => { 'transient' => marker },
                'rule_removed' => { 'before_signal' => ['DELETE FROM bot_signals'] } }
      gates['market_closed'] = { 'http' => { 'GET /v2/clock' => [closed_clock(SIGNAL_AT)] } } if vn['asset'] == 'stock'
      sizing = {
        'accepted' => { 'filled' => true },
        # Two calls in a row on one account: two orders, the second sized after the first was placed.
        'two_calls' => { 'calls' => 2, 'http' => { add => [vn['http'][add].first, vn['second']] } },
        'price_retried' => { 'http' => { vn['price_key'] => vn['price_transient'] + vn['http'][vn['price_key']] } },
        'price_failed' => { 'http' => { vn['price_key'] => vn['price_transient'] } },
        'price_refused' => { 'http' => { vn['price_key'] => vn['price_refused'] } },
        'balance_failed' => { 'http' => vn['balance_failed'] },
        'free_below_fixed' => { 'http' => vn['free'].(vkey == 'alpaca_stock' ? '0.1' : '0.001') },
        'no_holding' => { 'http' => vn['free'].('0') },
        'below_minimum' => { 'tiny' => true },
        'rejected' => { 'http' => { add => [vn['rejected']] } },
        'rejected_after_failure' => { 'http' => { add => [vn['rejected']] }, 'transactions' => [failed] },
        'rejected_after_skip' => { 'http' => { add => [vn['rejected']] }, 'transactions' => [failed, skipped] },
        'rejected_after_success' => { 'http' => { add => [vn['rejected']] }, 'transactions' => [failed, submitted] },
        'server_error' => { 'http' => { add => [vn['server_error']] } },
        'timeout' => { 'http' => { add => [POST_SEND] } },
        'unreadable' => { 'http' => { add => [{ 'status' => 200, 'body' => '<html>upstream</html>' }] } },
        'no_order_id' => { 'http' => { add => [vn['no_order_id']] } },
        'pre_send' => { 'http' => { add => [vn['pre_send']] } }
      }
      if vn['safe']
        accepted = vn['http'][add].first
        sizing['safe_then_accepted'] = { 'http' => { add => [vn['safe'], accepted] }, 'filled' => true }
        sizing['safe_then_rejected'] = { 'http' => { add => [vn['safe'], vn['rejected']] } }
        sizing['safe_twice'] = { 'http' => { add => [vn['safe'], vn['safe']] } }
      end
      cells = gates.map { |name, v| ['buy-fixed', name, v] } + rules.keys.product(sizing.to_a).map { |r, (name, v)| [r, name, v] }
      cells.each do |rule_name, name, v|
        rule = rules.fetch(rule_name).merge('enabled' => v.fetch('enabled', true))
        rule = rule.merge('amount' => rule['amount_type'] == 'fixed' ? '0.01' : '0.00001') if v['tiny']
        http = vn['http'].merge(v['filled'] ? vn['filled'].(rule['direction']) : {}).merge(v.fetch('http', {}))
        triggered = v.fetch('triggered_at', SIGNAL_AT)
        list << { 'name' => "signal-#{vkey}-#{rule_name}-#{name}", 'kind' => 'signal', 'venue' => vn.fetch('venue', vkey), 'asset' => vn['asset'],
                  'ticker' => vn['ticker'], 'rules' => [rule], 'status' => v['status'], 'key_status' => v['key_status'], 'no_key' => v['no_key'],
                  'transient' => v.fetch('transient', {}), 'transactions' => v.fetch('transactions', []), 'at' => SIGNAL_AT,
                  'calls' => Array.new(n = v.fetch('calls', 1)) { |i| { 'rule' => 0, 'triggered_at' => (Time.iso8601(triggered) - (31 * (n - 1 - i))).iso8601(6) } },
                  'before_signal' => v['before_signal'], 'script' => { vn['key'] => http }, 'tick' => false,
                  'poll' => (vn['order'] if v['filled']) }.compact
      end
    end
    list.tap { |l| raise "the signal grid has #{l.size} scenarios, not 420" unless l.size == 420 }
  end

  def grid(root, list)
    ActiveJob::Base.queue_adapter = :test
    list.each do |sc|
      dir = File.join(root, sc['name'])
      FileUtils.mkdir_p(dir)
      seed_dir = File.join(dir, '.seed')
      if ENV['RUST_GRID_CACHE'] && File.exist?(File.join(seed_dir, 'scenario.json'))
        %w[production.sqlite3 production_queue.sqlite3 scenario.json].each { |f| FileUtils.cp(File.join(seed_dir, f), File.join(dir, f)) }
        next
      end
      bot = case sc['kind']
            when 'index' then build_index(dir, sc)
            when 'venue_index' then build_venue_index(dir, sc)
            when 'signal' then build_signal(dir, sc)
            else sc['asset'] == 'stock' ? build_stock(dir, sc) : build(dir, sc)
            end
      enc = ActiveRecord::Encryption.config
      File.write(File.join(dir, 'scenario.json'),
                 JSON.pretty_generate({ 'encryption' => (%w[index venue_index].include?(sc['kind']) ? { 'primary_key' => Array(enc.primary_key).first, 'key_derivation_salt' => enc.key_derivation_salt } : nil), 'parity_scratch' => true, 'bot_id' => bot.id, 'at' => sc['at'], 'venue' => sc['venue'], 'script' => sc['script'],
                                        'tick' => sc['tick'], 'poll' => sc['poll'], 'recover_at' => sc['recover_at'], 'next_at' => sc['next_at'], 'rebalance_at' => sc['rebalance_at'],
                                        'between' => sc['between'], 'rails_cache' => sc['rails_cache'], 'check_at' => sc['check_at'],
                                        'conditions' => sc['conditions'], 'resync_at' => sc['resync_at'], 'trigger' => sc['trigger'],
                                        # Bot::SignalJob's arguments: the rule (by its id) and when the call arrived.
                                        'signals' => sc['calls']&.map { |c| { 'signal_id' => bot.bot_signals.order(:id)[c['rule']].id, 'triggered_at' => c['triggered_at'] } },
                                        'before_signal' => sc['before_signal'],
                                        # Bot::SmartIntervalable#smart_interval_minimum(:quote) as Rails computes it for the built bot.
                                        'rails_floor' => (sc['record_floor'] && bot.reload.smart_interval_minimum(:quote).then { |m| { 'value' => m[:value].to_f, 'reason' => m[:reason].to_s } }) }.compact))
      ActiveRecord::Base.connection_pool.disconnect!
      if ENV['RUST_GRID_CACHE']
        FileUtils.mkdir_p(seed_dir)
        %w[production.sqlite3 production_queue.sqlite3 scenario.json].each { |f| FileUtils.cp(File.join(dir, f), File.join(seed_dir, f)) }
      end
    end
    puts "built #{list.size} scenarios in #{root}"
  end

  def retry_of(bot_id)
    ActiveJob::Base.queue_adapter.enqueued_jobs.find do |j|
      j['job_class'] == 'Bot::ActionJob' && j['executions'].to_i.positive? && j['arguments'].to_s.include?("/#{bot_id}\"")
    end
  end

  # Bot::ActionJob at `at`, then the retries it enqueues for itself, as Solid Queue would run them. The mails each run
  # enqueued go to `mails` before the next run clears the queue.
  def tick_at(sc, at, mails)
    travel_to(at, with_usec: true) { Bot::ActionJob.perform_now(Bot.find(sc['bot_id'])) }
    (MAX_ATTEMPTS - 1).times do
      job = retry_of(sc['bot_id']) or break
      mails.concat(enqueued_mails)
      ActiveJob::Base.queue_adapter.enqueued_jobs.clear
      travel_to(Time.at(job[:at]), with_usec: true) { ActiveJob::Base.execute(job.stringify_keys) }
    end
  end

  # Bot::ResyncIndexCompositionJob at `at` and the retries its retry_on enqueues (3 attempts in all), each at its own time. A
  # raise ends the job, as Solid Queue's execution does.
  def resync_at(sc, at)
    ActiveJob::Base.queue_adapter.enqueued_jobs.clear
    run = -> { Bot::ResyncIndexCompositionJob.perform_now(Bot.find(sc['bot_id'])) }
    3.times do
      begin
        travel_to(at, with_usec: true) { run.() }
      rescue StandardError => e
        Rails.logger.info("resync raised: #{e.message}")
      end
      job = ActiveJob::Base.queue_adapter.enqueued_jobs.find { |j| j['job_class'] == 'Bot::ResyncIndexCompositionJob' } or break
      ActiveJob::Base.queue_adapter.enqueued_jobs.delete(job)
      at = Time.at(job[:at])
      run = -> { ActiveJob::Base.execute(job.stringify_keys) }
    end
  end

  # Bot::SignalJob for each call at `at`, one after another as the per-exchange semaphore runs them, each with the retries it
  # enqueues for itself (an unresolved placement waits 30 s), each at its own time. The jobs are serialized first, as the hook
  # enqueued them, so a rule removed afterwards (`before_signal`) is discarded on deserialization.
  def signal_calls(sc, mails)
    bot = Bot.find(sc['bot_id'])
    jobs = sc['signals'].map { |c| Bot::SignalJob.new(bot, BotSignal.find(c['signal_id']), Time.iso8601(c['triggered_at'])).serialize }
    (sc['before_signal'] || []).each { |sql| ActiveRecord::Base.connection.execute(sql) }
    jobs.each do |job|
      at = Time.iso8601(sc['at'])
      20.times do
        ActiveJob::Base.queue_adapter.enqueued_jobs.clear
        travel_to(at, with_usec: true) { ActiveJob::Base.execute(job) }
        mails.concat(enqueued_mails)
        retry_job = ActiveJob::Base.queue_adapter.enqueued_jobs.find { |j| j['job_class'] == 'Bot::SignalJob' } or break
        at = retry_job['scheduled_at'] ? Time.iso8601(retry_job['scheduled_at']) : Time.at(retry_job[:at] || retry_job['at'])
        job = retry_job.stringify_keys
      end
    end
    ActiveJob::Base.queue_adapter.enqueued_jobs.clear
  end

  def record(root)
    ActiveJob::Base.queue_adapter = :test # jobs are Rust's to replace; only retries are replayed below
    ActiveJob::Base.retry_jitter = 0.0
    # A real cache, as production has: development's :null_store would skip Exchange#get_*_price's 5 s cache.
    Rails.cache = ActiveSupport::Cache::MemoryStore.new
    # Exchange#with_transient_retry's backoff (a signal's reads): the times are the scenario's, never slept.
    Exchange.prepend(Module.new { def sleep(*) = nil })
    Honeymaker::Clients::Kraken.prepend(ScriptedKraken::Http)
    Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(ScriptedAlpaca::Adapter)
    Bot.prepend(Module.new do # broadcasts are UI side effects outside the comparison
      %i[broadcast_status_bar_update broadcast_new_order broadcast_updated_order broadcast_metrics_panel
         broadcast_quote_amount_limit_update broadcast_replace_to].each { |m| define_method(m) { |*, **| nil } }
    end)
    # A composition bot's own (Bot::ResyncIndexCompositionJob calls it): its module sits before Bot in the ancestors.
    Bot::Composition::Measurable.prepend(Module.new { def broadcast_metrics_panel(*, **) = nil })
    Dir[File.join(root, '*/scenario.json')].sort.each do |path|
      dir = File.dirname(path)
      next if ENV['RUST_GRID_CACHE'] && File.exist?(File.join(dir, 'rails.json'))
      $stderr.puts("recording #{File.basename(dir)}") # the watchdog's kill names the scenario that hung
      sc = JSON.parse(File.read(path))
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
      Rails.cache.clear # Exchanges::Kraken caches prices by exchange/ticker id, which every scenario shares
      ActiveJob::Base.queue_adapter.enqueued_jobs.clear
      alpaca = sc['venue'] == 'alpaca'
      honeymaker = VENUES[sc['venue']] # a venue scripted beneath honeymaker (Binance, KuCoin, ...)
      ScriptedKraken.http = alpaca || honeymaker ? {} : sc['script']['http']&.transform_values(&:dup)
      ScriptedKraken.sent = []
      ScriptedKrakenHttp.http = sc['script']['kraken_http']&.transform_values(&:dup)
      ScriptedKrakenHttp.sent = []
      ScriptedAlpaca.http = alpaca ? sc['script']['alpaca'].transform_values(&:dup) : {} # {}: any Alpaca call in a Kraken scenario is unscripted
      ScriptedAlpaca.sent = []
      VENUES.each do |key, row|
        row.scripted.http = key == sc['venue'] ? sc['script'][key].transform_values(&:dup) : {}
        row.scripted.sent = []
      end
      before = snapshot
      markers = Bot.find(sc['bot_id']).transient_data.slice(*MAIL_MARKERS)
      mails = []
      # What Transaction's after_commit enqueues for the amount limit (quote_amount_limitable.rb:94-106): Bot::StopJob runs at
      # once, at the moment of the phase that enqueued it (the engine stops in the fill's own transaction), and each mail is
      # listed, never delivered.
      settle = lambda do |time|
        mails.concat(enqueued_mails)
        jobs = ActiveJob::Base.queue_adapter.enqueued_jobs
        stops = jobs.select { |j| j['job_class'] == 'Bot::StopJob' }
        jobs.clear
        travel_to(time, with_usec: true) { stops.each { |j| ActiveJob::Base.execute(j.stringify_keys) } }
      end
      if (cached = sc['rails_cache']) # Rails' own one-minute clock cache, written before the tick (the clock_stale_cache divergence)
        exchange_id = Bot.find(sc['bot_id']).exchange_id
        travel_to(Time.iso8601(cached['at']), with_usec: true) { Rails.cache.write("exchange_#{exchange_id}_clock", cached['clock'], expires_in: 1.minute) }
        travel_back
      end
      # Every Bot::PriceLimitCheckJob the phase queued, at the time it was queued for.
      checks = []
      limit_checks = lambda do
        ActiveJob::Base.queue_adapter.enqueued_jobs.select { |j| j['job_class'] == 'Bot::PriceLimitCheckJob' }.map do |j|
          (j['scheduled_at'] ? Time.iso8601(j['scheduled_at']) : Time.at(j[:at] || j['at'])).utc.iso8601(6)
        end
      end
      # Plan CI T3b: what triggers a Bot::ResyncIndexCompositionJob (the same SQL runs on Rust's copy), then the job and the
      # retries it enqueues for itself, as Solid Queue would run them, before the tick.
      if sc['resync_at']
        (sc['trigger'] || []).each { |sql| ActiveRecord::Base.connection.execute(sql) }
        resync_at(sc, Time.iso8601(sc['resync_at']))
        mails.concat(enqueued_mails)
        ActiveJob::Base.queue_adapter.enqueued_jobs.clear
      end
      if sc.fetch('tick', true)
        tick_at(sc, Time.iso8601(sc['at']), mails)
        checks.concat(limit_checks.())
        settle.(Time.iso8601(sc['at']))
      end
      signal_calls(sc, mails) if sc['signals']
      if sc['check_at']
        ActiveJob::Base.queue_adapter.enqueued_jobs.clear
        check_time = Time.iso8601(sc['check_at'])
        begin
          travel_to(check_time, with_usec: true) { Bot::PriceLimitCheckJob.perform_now(Bot.find(sc['bot_id'])) }
        rescue StandardError
          nil # the job raised: its chain dies until Bot::RepairOrphanedBotsJob
        end
        checks.concat(limit_checks.())
        if ActiveJob::Base.queue_adapter.enqueued_jobs.any? { |j| j['job_class'] == 'Bot::ActionJob' }
          ActiveJob::Base.queue_adapter.enqueued_jobs.clear
          tick_at(sc, check_time, mails)
          checks.concat(limit_checks.())
        end
        settle.(check_time)
      end
      ActiveJob::Base.queue_adapter.enqueued_jobs.clear
      poll_error = nil
      if sc['poll'] # the follow-up poll Transaction enqueues for one order; its retries are not replayed
        order = Transaction.find_by(bot_id: sc['bot_id'], external_id: sc['poll']) or raise "#{dir}: missing poll order #{sc['poll']}, rows=#{Transaction.all.pluck(:external_id)}"
        travel_to(Time.iso8601(sc['at']) + 5, with_usec: true) do
          Bot::FetchAndUpdateOrderJob.perform_now(order, update_missed_quote_amount: true)
        rescue StandardError => e
          # A Kraken scenario must not fail its poll, except a listed unreadable-number divergence (honeymaker's strict
          # BigDecimal() raises on "garbage"): that raise is Rails' answer, recorded for rust/tests/parity.rs.
          raise unless alpaca || honeymaker || File.basename(dir).include?('-unreadable_')

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
        tick_at(sc, Time.iso8601(sc['next_at']), mails)
        settle.(Time.iso8601(sc['next_at']))
      end
      # Bot::EvaluateRebalancersJob's fires: one Bot::RebalanceJob each (it has no retry_on; a raise ends that run). The
      # FetchAndUpdateOrderJob it enqueues is not run: the next fire's refresh_pending_order! reads the order itself.
      (sc['rebalance_at'] || []).each do |time|
        ActiveJob::Base.queue_adapter.enqueued_jobs.clear
        travel_to(Time.iso8601(time), with_usec: true) do
          Bot::RebalanceJob.perform_now(Bot.find(sc['bot_id']))
        rescue StandardError => e
          Rails.logger.info("rebalance raised: #{e.message}")
        end
        settle.(Time.iso8601(time))
      end
      mails.concat(enqueued_mails)
      sent = if alpaca then ScriptedAlpaca.sent elsif honeymaker then honeymaker.scripted.sent
             elsif ScriptedKrakenHttp.http then ScriptedKrakenHttp.sent else ScriptedKraken.sent end
      out = { 'sent' => sent, 'changes' => diff(before, snapshot), 'mails' => mails.sort_by { |m| m['mail'] } }
      out['checks'] = checks if sc['conditions']
      # Only where the scenario seeded the Rust engine's markers: what Rails left of them.
      out['markers'] = Bot.find(sc['bot_id']).transient_data.slice(*MAIL_MARKERS) if markers.any?
      # Alpaca only: the funds notification (its column is excluded from the snapshot) and the follow-up's raise.
      out.merge!('funds_notified' => Bot.find(sc['bot_id']).last_end_of_funds_notification.present?, 'poll_error' => poll_error) if alpaca || honeymaker
      out['poll_error'] = poll_error if !alpaca && !honeymaker && poll_error
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
USAGE = 'usage: grid <root> | grid-signal <root> | grid-alpaca <root> | grid-basket <root> | grid-limit <root> | grid-binance <root> | grid-binance-us <root> | grid-kucoin <root> | grid-bitvavo <root> | grid-gemini <root> | grid-sell <root> | grid-venue-basket <root> | grid-venue-index <root> | grid-conditions <root> | grid-basket-sell <root> | grid-rebalance <root> | record <root> | web-save <dir>'.freeze
raise ArgumentError, USAGE unless root

case command
when 'grid' then Decisions.grid(root, Decisions.scenarios)
when 'grid-alpaca' then Decisions.grid(root, Decisions.alpaca_scenarios)
when 'grid-basket' then Decisions.grid(root, Decisions.basket_scenarios)
when 'grid-stock' then Decisions.grid(root, Decisions.stock_scenarios)
when 'grid-index' then Decisions.grid(root, Decisions.index_scenarios)
when 'grid-venue-index' then Decisions.grid(root, Decisions.venue_index_scenarios)
when 'grid-limit' then Decisions.grid(root, Decisions.limit_scenarios)
when 'grid-binance' then Decisions.grid(root, Decisions.binance_scenarios)
when 'grid-binance-us' then Decisions.grid(root, Decisions.binance_scenarios('binance_us'))
when 'grid-kucoin' then Decisions.grid(root, Decisions.kucoin_scenarios)
when 'grid-bitvavo' then Decisions.grid(root, Decisions.bitvavo_scenarios)
when 'grid-gemini' then Decisions.grid(root, Decisions.gemini_scenarios)
when 'grid-sell' then Decisions.grid(root, Decisions.sell_scenarios)
when 'grid-venue-basket' then Decisions.grid(root, Decisions.venue_basket_scenarios)
when 'grid-basket-sell' then Decisions.grid(root, Decisions.basket_sell_scenarios)
when 'grid-rebalance' then Decisions.grid(root, Decisions.rebalance_scenarios)
when 'grid-conditions' then Decisions.grid(root, Decisions.condition_scenarios)
when 'grid-signal' then Decisions.grid(root, Decisions.signal_scenarios)
when 'record' then Decisions.record(root)
when 'record-normalized'
  require_relative 'histories_normalized'
  Thread.current[:normalized_figure_rows] = true
  Decisions.record(root)
when 'web-save' then web_save(root)
else raise ArgumentError, USAGE
end
