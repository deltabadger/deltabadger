# BingX honeymaker signing oracle; every connection uses the test adapter.
require 'json'
require 'digest'
require 'base64'

class Unscripted < Exception; end # rubocop:disable Lint/InheritException

Net::HTTP.prepend(Module.new { def connect = raise(Unscripted, "real connection to #{address}") })

venue, out_path = 'bingx', ARGV.fetch(0)
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
out = { 'sources' => { 'script/rust/record_bingx_signing.rb' => Digest::SHA256.file(Rails.root.join('script/rust/record_bingx_signing.rb')).hexdigest },
        'gems' => gems }

connection = test_connection.(Honeymaker::Clients::BingX::URL, {code: 0, data: {orderId: '1', symbol: 'BTC-USDT'}})
cases = []
[%w[key secret], ['k','s3cr3t~/+=']].each_with_index do |(key, secret), i|
  uuid = '5f1c1f3e2b8f4c4e9a590d9c3f1b6a01'
  [ ['balances', ->(c) { c.get_raw_balances }],
    ['query', ->(c) { c.get_order(symbol: 'BTC-USDT', order_id: '1') }],
    ['query client', ->(c) { c.get_order(symbol: 'BTC-USDT', client_order_id: uuid) }],
    ['trades', ->(c) { c.get_trade_fills(symbol: 'BTC-USDT', start_time: 1787000000000, end_time: 1789871400750, limit: 500) }],
    ['deposits', ->(c) { c.deposit_history(start_time: 1787000000000, end_time: 1789871400750, offset: 0, limit: 1000) }],
    ['withdrawals', ->(c) { c.withdraw_history(start_time: 1787000000000, end_time: 1789871400750, offset: 0, limit: 1000) }],
    ['market quote', ->(c) { c.place_order(symbol: 'BTC-USDT', side: 'BUY', type: 'MARKET', quote_order_qty: '60', client_order_id: uuid) }],
    ['market base', ->(c) { c.place_order(symbol: 'BTC-USDT', side: 'SELL', type: 'MARKET', quantity: '0.001') }],
    ['limit', ->(c) { c.place_order(symbol: 'ETH-BTC', side: 'BUY', type: 'LIMIT', time_in_force: 'GTC', quantity: '1.2', price: '0.03512', client_order_id: uuid) }],
    ['cancel', ->(c) { c.cancel_order(symbol: 'BTC-USDT', order_id: '1') }],
    ['escaped', ->(c) { c.get_trade_fills(symbol: 'A B/~*é') }],
    ['symbols public', ->(c) { c.get_symbols }]
  ].each_with_index do |(name, call), j|
    c = Honeymaker::Clients::BingX.new(api_key: key, api_secret: secret)
    c.instance_variable_set(:@connection, connection)
    t = 1789871400750 + i*1000 + j
    c.define_singleton_method(:timestamp_ms) { t }
    call.call(c)
    cases << one.(name).merge('call'=>name, 'api_key'=>key, 'api_secret'=>secret, 'timestamp'=>t)
  end
end
out['sources']['app/models/exchanges/bingx.rb'] = Digest::SHA256.file(Rails.root.join('app/models/exchanges/bingx.rb')).hexdigest
out.merge!('errors'=>Exchanges::Bingx::ERRORS.transform_keys(&:to_s), 'network_transient'=>Client::NETWORK_TRANSIENT_PATTERNS, 'signing'=>cases)
File.write(out_path, "#{JSON.pretty_generate(out)}\n")
puts "BingX: #{cases.size} signed requests"
