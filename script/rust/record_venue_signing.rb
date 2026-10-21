# Records the signed requests honeymaker 0.12.5 (and hyperliquid-rb 0.1.1, which honeymaker's Hyperliquid client
# delegates to) sends, one section per venue (plan VB §1.8): KuCoin's KC-API-* headers (rust/tests/kucoin.rs), and for the
# venues whose signing is their own, Gemini's payload + HMAC-SHA384, Coinbase's per-request JWT (ES256 and EdDSA) and
# Hyperliquid's EIP-712 L1 action (rust/tests/venue_signing.rs, against venue::signers).
#   bin/rails runner script/rust/record_venue_signing.rb kucoin rust/tests/fixtures/kucoin_signing.json
#   bin/rails runner script/rust/record_venue_signing.rb bitvavo rust/tests/fixtures/bitvavo_signing.json
#   bin/rails runner script/rust/record_venue_signing.rb bitget rust/tests/fixtures/bitget_signing.json
#   bin/rails runner script/rust/record_venue_signing.rb mexc rust/tests/fixtures/mexc_signing.json
#   bin/rails runner script/rust/record_venue_signing.rb gemini rust/tests/fixtures/gemini_signing.json
#   bin/rails runner script/rust/record_venue_signing.rb coinbase rust/tests/fixtures/coinbase_signing.json
#   bin/rails runner script/rust/record_venue_signing.rb hyperliquid rust/tests/fixtures/hyperliquid_signing.json
# Every key below is a throwaway test key made for this file; clocks, nonces and SecureRandom are frozen. Nothing leaves
# the machine: the clients' connections are Faraday test adapters and any real connection raises.
require 'json'
require 'digest'
require 'base64'

class Unscripted < Exception; end # rubocop:disable Lint/InheritException

Net::HTTP.prepend(Module.new { def connect = raise(Unscripted, "real connection to #{address}") })

venue, out_path = ARGV.fetch(0), ARGV.fetch(1)
captured = []
test_connection = lambda do |url, answer|
  stubs = Faraday::Adapter::Test::Stubs.new do |s|
    %i[get post delete].each do |verb|
      s.public_send(verb, /.*/) do |env|
        captured << { 'method' => env.method.to_s.upcase, 'path' => env.url.path, 'query' => env.url.query,
                      'body' => env.request_body, 'headers' => env.request_headers.to_h.except('User-Agent') }
        [200, { 'Content-Type' => 'application/json' }, answer.to_json]
      end
    end
  end
  Faraday.new(url: url) do |f|
    f.request :json
    f.response :json
    f.adapter :test, stubs
  end
end
one = lambda do |name|
  raise "#{name}: #{captured.size} requests" unless captured.size == 1

  captured.pop
end

gems = %w[honeymaker hyperliquid-rb jwt rbnacl eth msgpack].to_h { |g| [g, Gem.loaded_specs.fetch(g).version.to_s] }
out = { 'sources' => { 'script/rust/record_venue_signing.rb' => Digest::SHA256.file(Rails.root.join('script/rust/record_venue_signing.rb')).hexdigest },
        'gems' => gems }

case venue
when 'kucoin'
  # Honeymaker::Clients::Kucoin's signed requests for rust/tests/kucoin.rs: the URL query, the body bytes and every header it
  # sends, under a frozen #timestamp_ms, for signed GETs (with and without a query), POSTs and DELETEs; and the bodies
  # Exchanges::Kucoin#set_market_order / #set_limit_order post.
  connection = test_connection.(Honeymaker::Clients::Kucoin::URL, { code: '200000', data: { orderId: '6716f0a2c1b6d10007a3b1c5' } })
  client = lambda do |key, secret, passphrase, ms|
    c = Honeymaker::Clients::Kucoin.new(api_key: key, api_secret: secret, passphrase: passphrase)
    c.instance_variable_set(:@connection, connection)
    c.define_singleton_method(:timestamp_ms) { ms }
    c
  end

  keys = [%w[6716e9b2a1c3f00001a2b3c4 4f1e8a2c-7b3d-4e6f-9a1b-2c3d4e5f6a7b my-passphrase],
          ['k', 's3cr3t~/+=', 'pass phrase/é']]
  cases = []
  record = lambda do |(key, secret, passphrase), ms, call, &blk|
    blk.call(client.call(key, secret, passphrase, ms))
    cases << one.(call).merge('call' => call, 'api_key' => key, 'api_secret' => secret, 'passphrase' => passphrase, 'timestamp' => ms)
  end

  ms = 1_789_871_400_750
  keys.each_with_index do |k, i|
    t = ms + (i * 1000)
    record.(k, t + 1, 'get_accounts') { |c| c.get_accounts(type: 'trade') }
    record.(k, t + 2, 'get_order') { |c| c.get_order(order_id: '6716f0a2c1b6d10007a3b1c5') }
    record.(k, t + 3, 'get_fills') do |c|
      c.get_fills(order_id: '6716f0a2c1b6d10007a3b1c5', start_at: 1_789_000_000_000, end_at: 1_789_604_800_000, limit: 500, current_page: 2)
    end
    record.(k, t + 4, 'get_accounts escaped') { |c| c.get_accounts(currency: 'A B/~*é', type: 'trade') }
    record.(k, t + 5, 'place_order market funds') do |c|
      c.place_order(client_oid: '5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01', side: 'buy', symbol: 'BTC-USDT', type: 'market', funds: '60.5')
    end
    record.(k, t + 6, 'place_order limit') do |c|
      c.place_order(client_oid: '5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01', side: 'sell', symbol: 'BTC-USDT', type: 'limit', size: '0.00093', price: '64150.5')
    end
    record.(k, t + 7, 'cancel_order') { |c| c.cancel_order(order_id: '6716f0a2c1b6d10007a3b1c5') }
  end

  # Exchanges::Kucoin's own order bodies: market by funds and by size, limit (a quote amount turned into a floored base size),
  # for both sides. Rails' random clientOid is pinned to one UUID, the one Rust sends as the intent id.
  Rails.configuration.dry_run = false # the test environment trades dry; the real set_*_order is what is recorded
  kucoin = Exchanges::Kucoin.new(name: 'KuCoin')
  kucoin.instance_variable_set(:@client, client.call(*keys.first, ms))
  SecureRandom.singleton_class.prepend(Module.new { def uuid = '5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01' })
  ticker = Ticker.new(exchange: kucoin, ticker: 'BTC-USDT', base: 'BTC', quote: 'USDT', minimum_base_size: BigDecimal('0.00001'),
                      minimum_quote_size: BigDecimal('0.1'), base_decimals: 8, quote_decimals: 6, price_decimals: 1)
  orders = []
  [[:market, :quote, '60.1234567', nil], [:market, :base, '0.000123456789', nil], [:limit, :base, '0.000935', '64150.57'],
   [:limit, :quote, '60', '64150.57']].each do |type, amount_type, amount, price|
    %i[buy sell].each do |side|
      result = if type == :market
                 kucoin.send(:set_market_order, ticker:, amount: BigDecimal(amount), amount_type:, side:)
               else
                 kucoin.send(:set_limit_order, ticker:, amount: BigDecimal(amount), amount_type:, side:, price: BigDecimal(price))
               end
      raise "#{type} #{side}: #{result.inspect}" unless result.success?

      sent = one.("#{type} #{side}")
      orders << { 'type' => type.to_s, 'side' => side.to_s, 'amount_type' => amount_type.to_s, 'amount' => amount, 'price' => price,
                  'body' => sent['body'], 'headers' => sent['headers'], 'timestamp' => ms }
    end
  end

  # Its own fixture shape (plan K): its Rails sources pinned, and what engine::venue_rules::KUCOIN ports beside the signing.
  out = {
    'sources' => %w[app/models/exchanges/kucoin.rb app/models/exchange.rb app/models/client.rb app/models/ticker.rb
                     script/rust/record_venue_signing.rb]
      .to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] },
    'gems' => { 'honeymaker' => Honeymaker::VERSION },
    'api_key' => keys.first,
    # What engine::venue_rules::KUCOIN ports: the venue's error lists, the network patterns Exchange#transient_error? adds
    # for every venue, and the placement-safe strings Exchange#placement_transient_error? starts from.
    'errors' => Exchanges::Kucoin::ERRORS.transform_keys(&:to_s),
    'network_transient' => Client::NETWORK_TRANSIENT_PATTERNS,
    'placement_safe' => Exchange::PLACEMENT_SAFE_TRANSIENT_ERRORS,
    'signing' => cases,
    'orders' => orders
  }
when 'bitvavo'
  # Honeymaker::Clients::Bitvavo's requests for rust/tests/bitvavo.rs (plan VB slice A3): the URL query, the body bytes and
  # every header it sends under a frozen #timestamp_ms, for signed GETs (with and without a query), POSTs, DELETEs (which sign
  # their query, where KuCoin's sign the path alone) and a public read.
  connection = test_connection.(Honeymaker::Clients::Bitvavo::URL, { orderId: '0b6d2f7e-1c4a-4b8e-9f3a-5d2c7e1a9b01', market: 'BTC-EUR' })
  keys = [%w[4a1f9c2e7b3d5a6f8e0c1b2d3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8c9d0e1f2a3 9f8e7d6c5b4a39281706f5e4d3c2b1a0f9e8d7c6b5a493827160f5e4d3c2b1a0f9],
          ['k', 's3cr3t~/+=']]
  cases = []
  ms = 1_789_871_400_750
  keys.each_with_index do |(key, secret), i|
    [['get_raw_balance', ->(c) { c.get_raw_balance }],
     ['get_order', ->(c) { c.get_order(market: 'BTC-EUR', order_id: '0b6d2f7e-1c4a-4b8e-9f3a-5d2c7e1a9b01') }],
     ['get_trades', ->(c) { c.get_trades(market: 'BTC-EUR', limit: 1000, trade_id_to: '1b2c3d4e-0000-4000-8000-000000000001') }],
     ['get_deposit_history', ->(c) { c.get_deposit_history(start_time: 0, end_time: 1_789_871_400_751, limit: 1000) }],
     ['get_raw_balance escaped', ->(c) { c.get_raw_balance(symbol: 'A B/~*é') }],
     ['place_order market amount', ->(c) { c.place_order(market: 'BTC-EUR', side: 'buy', order_type: 'market', amount: '0.00093748', client_order_id: '5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01') }],
     ['place_order market quote', ->(c) { c.place_order(market: 'BTC-EUR', side: 'sell', order_type: 'market', amount_quote: '60.5') }],
     ['place_order limit', ->(c) { c.place_order(market: 'ETH-EUR', side: 'buy', order_type: 'limit', amount: '1.5', price: '2400', client_order_id: '5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01') }],
     ['cancel_order', ->(c) { c.cancel_order(market: 'BTC-EUR', order_id: '00000000-0000-0000-0000-000000000000') }],
     ['get_markets public', ->(c) { c.get_markets }]].each_with_index do |(call, blk), j|
      c = Honeymaker::Clients::Bitvavo.new(api_key: key, api_secret: secret)
      c.instance_variable_set(:@connection, connection)
      t = ms + (i * 1000) + j
      c.define_singleton_method(:timestamp_ms) { t }
      blk.call(c)
      cases << one.(call).merge('call' => call, 'api_key' => key, 'api_secret' => secret, 'timestamp' => t)
    end
  end
  out['sources']['app/models/exchanges/bitvavo.rb'] = Digest::SHA256.file(Rails.root.join('app/models/exchanges/bitvavo.rb')).hexdigest
  # What engine::venue_rules::BITVAVO ports beside the signing.
  out.merge!('errors' => Exchanges::Bitvavo::ERRORS.transform_keys(&:to_s), 'network_transient' => Client::NETWORK_TRANSIENT_PATTERNS,
             'placement_safe' => Exchange::PLACEMENT_SAFE_TRANSIENT_ERRORS, 'signing' => cases)
when 'bitget'
  # Honeymaker::Clients::Bitget's requests for rust/tests/bitget.rs (plan VB slice A4): the URL query, the body bytes and every
  # header it sends under a frozen #timestamp_ms, for signed GETs (with and without a query, an escaped one), POSTs (the JSON
  # body as built, nils compacted) and the public reads.
  connection = test_connection.(Honeymaker::Clients::Bitget::URL, { code: '00000', msg: 'success', data: { orderId: '1234567890123456789' } })
  keys = [%w[bg_4a1f9c2e7b3d5a6f8e0c1b2d3f4a5b6c 9f8e7d6c5b4a39281706f5e4d3c2b1a0f9e8d7c6b5a493827160f5e4d3c2b1a0 my-passphrase],
          ['k', 's3cr3t~/+=', 'pass phrase/é']]
  cases = []
  ms = 1_789_871_400_750
  keys.each_with_index do |(key, secret, passphrase), i|
    [['get_account_assets', ->(c) { c.get_account_assets }],
     ['get_order', ->(c) { c.get_order(order_id: '1234567890123456789') }],
     ['get_order client_oid', ->(c) { c.get_order(client_oid: '5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01') }],
     ['get_fills', ->(c) { c.get_fills(start_time: '1789000000000', end_time: '1796689600000', limit: '100', id_less_than: '1234') }],
     ['deposit_list', ->(c) { c.deposit_list(start_time: '1522540800000', end_time: '1530230400000', limit: '100') }],
     ['get_account_assets escaped', ->(c) { c.get_account_assets(coin: 'A B/~*é') }],
     ['place_order market buy', ->(c) { c.place_order(symbol: 'BTCUSDT', side: 'buy', order_type: 'market', force: 'ioc', size: '60.5', client_oid: '5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01') }],
     ['place_order limit', ->(c) { c.place_order(symbol: 'ETHBTC', side: 'sell', order_type: 'limit', force: 'gtc', price: '0.03512', size: '1.2') }],
     ['cancel_order', ->(c) { c.cancel_order(symbol: 'BTCUSDT', order_id: '0') }],
     ['get_tickers public', ->(c) { c.get_tickers(symbol: 'BTCUSDT') }],
     ['get_orderbook public', ->(c) { c.get_orderbook(symbol: 'BTCUSDT', limit: 1) }]].each_with_index do |(call, blk), j|
      c = Honeymaker::Clients::Bitget.new(api_key: key, api_secret: secret, passphrase: passphrase)
      c.instance_variable_set(:@connection, connection)
      t = ms + (i * 1000) + j
      c.define_singleton_method(:timestamp_ms) { t }
      blk.call(c)
      cases << one.(call).merge('call' => call, 'api_key' => key, 'api_secret' => secret, 'passphrase' => passphrase, 'timestamp' => t)
    end
  end
  out['sources']['app/models/exchanges/bitget.rb'] = Digest::SHA256.file(Rails.root.join('app/models/exchanges/bitget.rb')).hexdigest
  out['sources']['app/models/exchange.rb'] = Digest::SHA256.file(Rails.root.join('app/models/exchange.rb')).hexdigest
  # What engine::venue_rules::BITGET ports beside the signing.
  out.merge!('errors' => Exchanges::Bitget::ERRORS.transform_keys(&:to_s), 'network_transient' => Client::NETWORK_TRANSIENT_PATTERNS,
             'placement_safe' => Exchange::PLACEMENT_SAFE_TRANSIENT_ERRORS, 'signing' => cases)
when 'mexc'
  # Honeymaker::Clients::Mexc's requests for rust/tests/mexc.rs (plan VB slice A5): the URL query (timestamp and signature
  # included), the body and every header it sends under a frozen #timestamp_ms, for signed GETs, POSTs and DELETEs (every
  # param in the query, as Binance's) and a public read.
  connection = test_connection.(Honeymaker::Clients::Mexc::URL, { orderId: 'C02__443776347957968896', symbol: 'BTCUSDT', type: 'MARKET', side: 'BUY',
                                                                   status: 'NEW', price: '0', origQty: '0', executedQty: '0', cummulativeQuoteQty: '0' })
  keys = [%w[mx0vglBqh6abc12345 0123456789abcdef0123456789abcdef], ['k', 's3cr3t~/+=']]
  cases = []
  ms = 1_789_871_400_750
  # MEXC's 32-character limit: the intent UUID's reversible hex form.
  uuid = '5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01'.delete('-')
  keys.each_with_index do |(key, secret), i|
    [['account_information', ->(c) { c.account_information }],
     ['query_order', ->(c) { c.query_order(symbol: 'BTCUSDT', order_id: 'C02__443776347957968896') }],
     ['query_order client', ->(c) { c.query_order(symbol: 'BTCUSDT', orig_client_order_id: uuid) }],
     ['account_trade_list', ->(c) { c.account_trade_list(symbol: 'BTCUSDT', start_time: 1_787_000_000_000, end_time: 1_789_871_400_750, limit: 100) }],
     ['deposit_history', ->(c) { c.deposit_history(start_time: 1_789_000_000_000, end_time: 1_789_604_800_000, limit: 1000) }],
     ['withdraw_history', ->(c) { c.withdraw_history(start_time: 1_789_000_000_000, end_time: 1_789_604_800_000, limit: 1000) }],
     ['new_order market quote', ->(c) { c.new_order(symbol: 'BTCUSDT', side: 'BUY', type: 'MARKET', quote_order_qty: '60.5', new_client_order_id: uuid) }],
     ['new_order market base', ->(c) { c.new_order(symbol: 'BTCUSDT', side: 'SELL', type: 'MARKET', quantity: '0.000937') }],
     ['new_order limit', ->(c) { c.new_order(symbol: 'ETHBTC', side: 'BUY', type: 'LIMIT', time_in_force: 'GTC', price: '0.03512', quantity: '1.2', new_client_order_id: uuid) }],
     ['cancel_order', ->(c) { c.cancel_order(symbol: 'BTCUSDT', order_id: 'C02__443776347957968896') }],
     ['account_trade_list escaped', ->(c) { c.account_trade_list(symbol: 'A B/~*é') }],
     ['exchange_information public', ->(c) { c.exchange_information }]].each_with_index do |(call, blk), j|
      c = Honeymaker::Clients::Mexc.new(api_key: key, api_secret: secret)
      c.instance_variable_set(:@connection, connection)
      t = ms + (i * 1000) + j
      c.define_singleton_method(:timestamp_ms) { t }
      blk.call(c)
      cases << one.(call).merge('call' => call, 'api_key' => key, 'api_secret' => secret, 'timestamp' => t)
    end
  end
  out['sources']['app/models/exchanges/mexc.rb'] = Digest::SHA256.file(Rails.root.join('app/models/exchanges/mexc.rb')).hexdigest
  # What engine::venue_rules::MEXC ports beside the signing.
  out.merge!('errors' => Exchanges::Mexc::ERRORS.transform_keys(&:to_s), 'network_transient' => Client::NETWORK_TRANSIENT_PATTERNS,
             'placement_safe' => Exchange::PLACEMENT_SAFE_TRANSIENT_ERRORS, 'order_gone_codes' => Exchanges::Mexc::ORDER_GONE_CODES, 'signing' => cases)
when 'bybit'
  connection = test_connection.(Honeymaker::Clients::Bybit::URL, { retCode: 0, result: { orderId: '123', list: [] } })
  cases = []
  [%w[k secret], ['api-key', 's3cr3t~/+=']].each do |key, secret|
    [[:wallet_balance, { account_type: 'UNIFIED' }],
     [:wallet_balance, { account_type: 'UNIFIED', coin: 'A B/~*é' }],
     [:get_order, { category: 'spot', order_id: '123' }],
     [:get_order, { category: 'spot', order_link_id: '5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01' }],
     [:order_history, { category: 'spot', order_id: '123', start_time: 1789000000000, end_time: 1789604800000 }],
     [:execution_list, { category: 'spot', cursor: 'a+/=' }],
     [:deposit_records, { start_time: 1789000000000 }],
     [:internal_deposit_records, { cursor: 'b+/=' }],
     [:withdraw_records, { withdraw_type: 2 }],
     [:create_order, { category: 'spot', symbol: 'BTCUSDT', side: 'Buy', order_type: 'Market', qty: '60.0', market_unit: 'quoteCoin', order_link_id: '5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01' }],
     [:create_order, { category: 'spot', symbol: 'BTCUSDT', side: 'Sell', order_type: 'Limit', qty: '0.1', price: '60000.0', time_in_force: 'GTC' }],
     [:cancel_order, { category: 'spot', symbol: 'BTCUSDT', order_id: '0' }],
     [:tickers, { category: 'spot' }], [:orderbook, { category: 'spot', symbol: 'BTCUSDT', limit: 1 }]].each do |call, args|
      c = Honeymaker::Clients::Bybit.new(api_key: key, api_secret: secret)
      c.instance_variable_set(:@connection, connection)
      c.define_singleton_method(:timestamp_ms) { 1789871400750 }
      c.public_send(call, **args)
      cases << one.(call).merge('call' => call, 'api_key' => key, 'api_secret' => secret, 'timestamp' => 1789871400750)
    end
  end
  %w[app/models/exchanges/bybit.rb app/models/exchange.rb].each { |file| out['sources'][file] = Digest::SHA256.file(Rails.root.join(file)).hexdigest }
  out.merge!('errors' => Exchanges::Bybit::ERRORS.transform_keys(&:to_s), 'network_transient' => Client::NETWORK_TRANSIENT_PATTERNS,
             'placement_safe' => Exchange::PLACEMENT_SAFE_TRANSIENT_ERRORS, 'signing' => cases)
when 'gemini'
  key = 'account-Zq8XtV2bLm4N7cRk'
  secret = '3xQnTq9hKpZ2vYw8RcLm5Fd7JgBs'
  connection = test_connection.(Honeymaker::Clients::Gemini::URL, { order_id: '106817811', result: 'ok' })
  nonce = 1_789_871_400_750
  cases = []
  record = lambda do |call, &blk|
    nonce += 1
    c = Honeymaker::Clients::Gemini.new(api_key: key, api_secret: secret)
    c.instance_variable_set(:@connection, connection)
    n = nonce
    c.define_singleton_method(:timestamp_ms) { n }
    blk.call(c)
    cases << one.(call).merge('call' => call, 'nonce' => n)
  end
  # What Exchanges::Gemini sends (gemini.rb): the IOC "market" order, a limit, an order read, the key-check cancel
  # probe (order_id 0, an Integer), the ledger walks, and an unescaped-text case for the JSON encoder.
  record.('get_raw_balances', &:get_raw_balances)
  record.('new_order ioc') do |c|
    c.new_order(symbol: 'btcusd', amount: '0.00012', price: '64150.12', side: 'buy', type: 'exchange limit',
                options: ['immediate-or-cancel'])
  end
  record.('new_order limit') { |c| c.new_order(symbol: 'ethusd', amount: '1.5', price: '2400', side: 'sell', type: 'exchange limit') }
  record.('new_order client id') do |c|
    c.new_order(symbol: 'dogeusd', amount: '10', price: '0.1', side: 'buy', type: 'exchange limit',
                client_order_id: "5f1c1f3e-<&>/\"é   \\x", options: ['immediate-or-cancel'])
  end
  record.('order_status') { |c| c.order_status(order_id: '106817811') }
  record.('cancel_order probe') { |c| c.cancel_order(order_id: 0) }
  record.('cancel_order') { |c| c.cancel_order(order_id: '106817811') }
  record.('get_my_trades') { |c| c.get_my_trades(timestamp: 1_789_000_000_001, limit_trades: 500) }
  record.('get_transfers_v2') { |c| c.get_transfers_v2(timestamp: 1_789_000_000_001, limit_transfers: 50) }
  out.merge!('api_key' => key, 'api_secret' => secret, 'signing' => cases)

when 'coinbase'
  es256 = <<~PEM
    -----BEGIN EC PRIVATE KEY-----
    MHcCAQEEIMFTz7EqPCw/LWAEfvFmbyk59sPmhzbnkj6aV7BoJqu0oAoGCCqGSM49
    AwEHoUQDQgAEb3vZdG1GH6yn5IcQnkHYCkWhJOvHJS93DJisoZR2O0hubksq23ml
    EsxmvwYgoIA3BO1Vfbohnb1Y/FjSIXax5A==
    -----END EC PRIVATE KEY-----
  PEM
  ed25519 = 'r/CIE8TkMjoZwqjH9imoUUOWO3DT0Gz6lwM1uWc2oXSy5AdIXS0Ofu8cSyjfY1beI3BPc9ls+Z1vhczQ2lwgQw=='
  key_name = 'organizations/0f8e7d6c-5b4a-4321-9876-0123456789ab/apiKeys/fedcba98-7654-4321-8abc-def012345678'
  keys = { 'es256' => es256, 'es256 pasted with CRLF' => es256.gsub("\n", "\r\n"), 'ed25519' => ed25519,
           'ed25519 wrapped' => "#{ed25519[0, 40]}\n#{ed25519[40..]}\n", 'ed25519 seed only' => Base64.strict_encode64(Base64.decode64(ed25519)[0, 32]),
           'ed25519 too short' => Base64.strict_encode64('x' * 31), 'es256 unreadable' => "-----BEGIN EC PRIVATE KEY-----\nnot a key\n-----END EC PRIVATE KEY-----\n" }
  connection = test_connection.(Honeymaker::Clients::Coinbase::URL, { success: true })
  now = Time.at(1_789_871_400).utc
  nonces = %w[6b1f0c2a9e8d7f6e5d4c3b2a19087f6e 0123456789abcdef0123456789abcdef].cycle
  Time.singleton_class.prepend(Module.new { define_method(:now) { |*a, **k| a.empty? && k.empty? && Thread.current[:frozen_now] || super(*a, **k) } })
  SecureRandom.singleton_class.prepend(Module.new { define_method(:hex) { |*a| Thread.current[:frozen_now] ? nonces.next : super(*a) } })
  cases = []
  keys.each do |label, secret|
    c = Honeymaker::Clients::Coinbase.new(api_key: key_name, api_secret: secret)
    c.instance_variable_set(:@connection, connection)
    [['list_accounts', ->(cl) { cl.list_accounts(limit: 250) }],
     ['get_api_key_permissions', ->(cl) { cl.get_api_key_permissions }],
     ['list_orders batch', ->(cl) { cl.list_orders(order_ids: %w[a1 b2], limit: 100) }],
     ['create_order', lambda { |cl|
       cl.create_order(client_order_id: '5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01', product_id: 'BTC-USD', side: 'BUY',
                       order_configuration: { market_market_ioc: { quote_size: '10.5' } })
     }],
     ['cancel_orders', ->(cl) { cl.cancel_orders(order_ids: ['a1']) }]].each_with_index do |(call, blk), i|
      next if i.positive? && label.start_with?('ed25519 too', 'es256 unreadable')

      Thread.current[:frozen_now] = now + i
      begin
        blk.call(c)
      ensure
        Thread.current[:frozen_now] = nil
      end
      r = one.(call)
      auth = r['headers']['Authorization']
      jwt = auth&.delete_prefix('Bearer ')
      head, payload, sig = jwt&.split('.')
      cases << { 'key' => label, 'call' => call, 'method' => r['method'], 'path' => r['path'], 'now' => (now + i).to_i,
                 'headers' => r['headers'], 'jwt' => jwt,
                 'jwt_header' => head && Base64.urlsafe_decode64(head), 'jwt_payload' => payload && Base64.urlsafe_decode64(payload),
                 'signature' => sig }
    end
  end
  ec = OpenSSL::PKey::EC.new(es256)
  out.merge!('api_key' => key_name, 'secrets' => keys,
             'es256_public_key' => ec.public_key.to_bn.to_s(16).downcase, 'signing' => cases)

when 'hyperliquid'
  require 'hyperliquid'
  agent = '0x7ee1a7025c514d5d7eda7a80f501c77fb6ffea7e3858ee01fba642229613f450'
  account = '0x00000000000000000000000000000000000000a1'
  assets = { 'PURR/USDC' => 10_000, '@1' => 10_001, 'HYPE/USDC' => 10_107, 'BTC' => 0, 'X127' => 127, 'X128' => 128,
             'X255' => 255, 'X256' => 256, 'X65535' => 65_535, 'X65536' => 65_536 }
  # The honeymaker client, as Rails builds it (proxy omitted: it is a connection setting, not part of the bytes), with
  # hyperliquid-rb's asset lookup answered from the table above and its /exchange transport captured.
  client = Honeymaker::Clients::Hyperliquid.new(api_key: account, api_secret: agent)
  exchange = client.send(:exchange_client)
  exchange.info.define_singleton_method(:name_to_asset) { |coin| assets.fetch(coin) }
  exchange.instance_variable_get(:@transport)
          .instance_variable_set(:@connection, test_connection.('https://api.hyperliquid.xyz', { status: 'ok' }))
  sign_spy = []
  signer = exchange.instance_variable_get(:@signer)
  signer.singleton_class.prepend(Module.new do
    define_method(:sign_l1_action) do |action, **kw|
      sign_spy << { 'msgpack' => MessagePack.pack(action).unpack1('H*'), 'hash' => action_hash(action, **kw).unpack1('H*') }
      super(action, **kw)
    end
  end)
  frozen_ms = 1_789_871_400_750
  Time.singleton_class.prepend(Module.new { define_method(:now) { |*a, **k| a.empty? && k.empty? && Thread.current[:frozen_ms] ? Time.at(Thread.current[:frozen_ms] / 1000r) : super(*a, **k) } })
  cases = []
  run = lambda do |name, ms, &blk|
    Thread.current[:frozen_ms] = ms
    begin
      blk.call
    ensure
      Thread.current[:frozen_ms] = nil
    end
    r = one.(name)
    spy = sign_spy.pop
    body = JSON.parse(r['body'])
    cases << { 'call' => name, 'path' => r['path'], 'body' => r['body'], 'nonce' => body['nonce'], 'action' => body['action'],
               'msgpack' => spy['msgpack'], 'action_hash' => spy['hash'], 'signature' => body['signature'] }
  end
  # 1. Rails' wire: honeymaker's #order (GTC, no cloid), buy and sell, BigDecimal size and price as Rails passes them.
  run.('order', frozen_ms) { client.order(coin: 'PURR/USDC', is_buy: true, size: BigDecimal('1234.5'), limit_px: BigDecimal('0.19876')) }
  run.('order sell', frozen_ms + 10) { client.order(coin: 'HYPE/USDC', is_buy: false, size: BigDecimal('0.37'), limit_px: BigDecimal('41.234')) }
  # 2. Rust's wire: the gem's own order(cloid:) path.
  cloid = Hyperliquid::Cloid.from_str('0x5F1C1F3E2B8F4C4E9A590D9C3F1B6A01')
  run.('order cloid', frozen_ms + 20) do
    client.send(:with_status_rescue) do
      exchange.order('PURR/USDC', is_buy: true, sz: BigDecimal('10'), limit_px: BigDecimal('0.2'), order_type: { limit: { tif: 'Gtc' } }, cloid: cloid)
    end
  end
  # 3. cancels, with an oid past 32 bits.
  run.('cancel', frozen_ms + 30) { client.cancel(coin: 'PURR/USDC', oid: 41_234_567_890) }
  run.('cancel small', frozen_ms + 40) { client.cancel(coin: '@1', oid: 7) }
  # 4. every msgpack int width for the asset, and float_to_wire edges.
  assets.each_with_index do |(coin, _), i|
    run.("order asset #{coin}", frozen_ms + 100 + i) do
      client.order(coin:, is_buy: i.even?, size: BigDecimal('100'), limit_px: BigDecimal('1670.1'))
    end
  end
  [%w[0.00000001 99999.999], %w[12345678.12345678 0.0147], %w[123456789012 1E+5], %w[0.1 30000.5]].each_with_index do |(sz, px), i|
    run.("order wire #{sz} #{px}", frozen_ms + 200 + i) do
      client.order(coin: 'BTC', is_buy: true, size: BigDecimal(sz), limit_px: BigDecimal(px))
    end
  end
  # 5. an order and a cancel signed in the same ms: honeymaker's process-wide nonce gives them consecutive values.
  run.('same ms order', frozen_ms + 1000) { client.order(coin: 'PURR/USDC', is_buy: true, size: BigDecimal('50'), limit_px: BigDecimal('0.2')) }
  run.('same ms cancel', frozen_ms + 1000) { client.cancel(coin: 'PURR/USDC', oid: 41_234_567_891) }
  raise 'same-ms nonces are not consecutive' unless cases[-1]['nonce'] == cases[-2]['nonce'] + 1

  wire = %w[0 1 100 100.0 1670.1 0.0147 0.00000001 0.000000005 0.000000015 0.123456789 1.000000001 99999999.99999999
            12345678.12345678 123456789.12345678 123456789012.34567 1234567890123.5 100000000000000000 0.30000000000000004
            -1.5 -0.000000001 1E+20 1E-8 41.234 64150.12345678].map do |x|
    { 'x' => x, 'wire' => Hyperliquid::Utils.float_to_wire(BigDecimal(x)) }
  rescue Hyperliquid::SigningError => e
    { 'x' => x, 'error' => e.message }
  end
  out.merge!('agent_key' => agent, 'assets' => assets, 'signing' => cases, 'float_to_wire' => wire)
else
  raise "unknown venue #{venue}"
end

File.write(out_path, "#{JSON.pretty_generate(out)}\n")
puts "#{venue}: #{out['signing'].size} signed requests"
