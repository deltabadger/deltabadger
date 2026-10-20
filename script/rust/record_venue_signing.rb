# Records the signed requests honeymaker 0.12.5 (and hyperliquid-rb 0.1.1, which honeymaker's Hyperliquid client
# delegates to) sends for the venues whose signing is their own: Gemini's payload + HMAC-SHA384, Coinbase's per-request
# JWT (ES256 and EdDSA) and Hyperliquid's EIP-712 L1 action. rust/tests/venue_signing.rs replays them against
# venue::signers.
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
