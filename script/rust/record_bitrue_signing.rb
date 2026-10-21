# Bitrue honeymaker signing oracle; every connection uses the test adapter.
require 'json'
require 'digest'
require 'base64'

class Unscripted < Exception; end # rubocop:disable Lint/InheritException

Net::HTTP.prepend(Module.new { def connect = raise(Unscripted, "real connection to #{address}") })

venue, out_path = 'bitrue', ARGV.fetch(0)
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
out = { 'sources' => { 'script/rust/record_bitrue_signing.rb' => Digest::SHA256.file(Rails.root.join('script/rust/record_bitrue_signing.rb')).hexdigest },
        'gems' => gems }

  # Honeymaker::Clients::Bitrue's requests for rust/tests/bitrue.rs (plan VB slice A6): the URL query (timestamp and signature
  # included), the body and every header it sends under a frozen #timestamp_ms, for signed GETs, POSTs and DELETEs (every
  # param in the query, as Binance's) and a public read.
  connection = test_connection.(Honeymaker::Clients::Bitrue::URL, { orderId: '443776347957968896', symbol: 'BTCUSDT', type: 'MARKET', side: 'BUY',
                                                                   status: 'NEW', price: '0', origQty: '0', executedQty: '0', cummulativeQuoteQty: '0' })
  keys = [%w[mx0vglBqh6abc12345 0123456789abcdef0123456789abcdef], ['k', 's3cr3t~/+=']]
  cases = []
  ms = 1_789_871_400_750
  # Neither official spot reference states a limit; use the reversible 32-character UUID form.
  uuid = '5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01'.delete('-')
  keys.each_with_index do |(key, secret), i|
    [['account_information', ->(c) { c.account_information }],
     ['query_order', ->(c) { c.query_order(symbol: 'BTCUSDT', order_id: '443776347957968896') }],
     ['query_order client', ->(c) { c.query_order(symbol: 'BTCUSDT', orig_client_order_id: uuid) }],
     ['account_trade_list', ->(c) { c.account_trade_list(symbol: 'BTCUSDT', start_time: 1_787_000_000_000, end_time: 1_789_871_400_750, limit: 100) }],
     ['deposit_history', ->(c) { c.deposit_history(coin: 'btc', status: 1, offset: 0, start_time: 1_789_000_000_000, end_time: 1_789_604_800_000, limit: 1000) }],
     ['withdraw_history', ->(c) { c.withdraw_history(coin: 'usdt', status: 5, offset: 0, start_time: 1_789_000_000_000, end_time: 1_789_604_800_000, limit: 1000) }],
     ['new_order market buy', ->(c) { c.new_order(symbol: 'BTCUSDT', side: 'BUY', type: 'MARKET', quantity: '0.000937', new_client_order_id: uuid) }],
     ['new_order market base', ->(c) { c.new_order(symbol: 'BTCUSDT', side: 'SELL', type: 'MARKET', quantity: '0.000937') }],
     ['new_order limit', ->(c) { c.new_order(symbol: 'ETHBTC', side: 'BUY', type: 'LIMIT', time_in_force: 'GTC', price: '0.03512', quantity: '1.2', new_client_order_id: uuid) }],
     ['cancel_order', ->(c) { c.cancel_order(symbol: 'BTCUSDT', order_id: '443776347957968896') }],
     ['account_trade_list escaped', ->(c) { c.account_trade_list(symbol: 'A B/~*é') }],
     ['exchange_information public', ->(c) { c.exchange_information }]].each_with_index do |(call, blk), j|
      c = Honeymaker::Clients::Bitrue.new(api_key: key, api_secret: secret)
      c.instance_variable_set(:@connection, connection)
      t = ms + (i * 1000) + j
      c.define_singleton_method(:timestamp_ms) { t }
      blk.call(c)
      cases << one.(call).merge('call' => call, 'api_key' => key, 'api_secret' => secret, 'timestamp' => t)
    end
  end
  out['sources']['app/models/exchanges/bitrue.rb'] = Digest::SHA256.file(Rails.root.join('app/models/exchanges/bitrue.rb')).hexdigest
  # What engine::venue_rules::Bitrue ports beside the signing.
  out.merge!('errors' => Exchanges::Bitrue::ERRORS.transform_keys(&:to_s), 'network_transient' => Client::NETWORK_TRANSIENT_PATTERNS,
             'placement_safe' => Exchange::PLACEMENT_SAFE_TRANSIENT_ERRORS + Exchanges::Bitrue::ERRORS.fetch(:placement_safe_transient), 'order_gone_codes' => Exchanges::Bitrue::ORDER_GONE_CODE, 'signing' => cases)
File.write(out_path, "#{JSON.pretty_generate(out)}\n")
puts "#{venue}: #{out['signing'].size} signed requests"
