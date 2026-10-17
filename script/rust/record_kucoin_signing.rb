# Records Honeymaker::Clients::Kucoin's signed requests for rust/tests/kucoin.rs: the URL query, the body bytes and every header
# it sends, under a frozen #timestamp_ms, for signed GETs (with and without a query), POSTs and DELETEs; and the bodies
# Exchanges::Kucoin#set_market_order / #set_limit_order post.
#   bin/rails runner script/rust/record_kucoin_signing.rb rust/tests/fixtures/kucoin_signing.json
# Nothing leaves the machine: the client's connection is a Faraday test adapter with the gem's own JSON middleware, and any
# real connection raises. Re-run (script/rust/repin) when a pinned file or the honeymaker version changes.
require 'json'
require 'digest'

class Unscripted < Exception; end # rubocop:disable Lint/InheritException

Net::HTTP.prepend(Module.new { def connect = raise(Unscripted, "real connection to #{address}") })

captured = []
stubs = Faraday::Adapter::Test::Stubs.new do |s|
  %i[get post delete].each do |verb|
    s.public_send(verb, /.*/) do |env|
      captured << { 'method' => env.method.to_s.upcase, 'path' => env.url.path, 'query' => env.url.query,
                    'body' => env.request_body, 'headers' => env.request_headers.to_h.except('User-Agent') }
      [200, { 'Content-Type' => 'application/json' }, { code: '200000', data: { orderId: '6716f0a2c1b6d10007a3b1c5' } }.to_json]
    end
  end
end
connection = Faraday.new(url: Honeymaker::Clients::Kucoin::URL) do |f|
  f.request :json
  f.response :json
  f.adapter :test, stubs
end

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
  captured.clear
  blk.call(client.call(key, secret, passphrase, ms))
  raise "#{call}: #{captured.size} requests" unless captured.size == 1

  cases << captured.first.merge('call' => call, 'api_key' => key, 'api_secret' => secret, 'passphrase' => passphrase, 'timestamp' => ms)
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
    captured.clear
    result = if type == :market
               kucoin.send(:set_market_order, ticker:, amount: BigDecimal(amount), amount_type:, side:)
             else
               kucoin.send(:set_limit_order, ticker:, amount: BigDecimal(amount), amount_type:, side:, price: BigDecimal(price))
             end
    raise "#{type} #{side}: #{result.inspect}" unless result.success? && captured.size == 1

    orders << { 'type' => type.to_s, 'side' => side.to_s, 'amount_type' => amount_type.to_s, 'amount' => amount, 'price' => price,
                'body' => captured.first['body'], 'headers' => captured.first['headers'], 'timestamp' => ms }
  end
end

out = {
  'sources' => %w[app/models/exchanges/kucoin.rb app/models/exchange.rb app/models/client.rb app/models/ticker.rb
                   script/rust/record_kucoin_signing.rb]
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
File.write(ARGV.fetch(0), "#{JSON.pretty_generate(out)}\n")
puts "#{cases.size} signed requests, #{orders.size} order bodies"
