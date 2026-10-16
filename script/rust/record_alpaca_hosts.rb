# Records which Alpaca host each Rails call reaches for each key mode, for rust/tests/alpaca_hosts.rs.
#   bin/rails runner script/rust/record_alpaca_hosts.rb rust/tests/fixtures/alpaca_hosts.json
# Run in a clean export (git archive, no .env, `db:schema:load`, script/rust/build_assets.sh). Nothing leaves the machine:
# every Alpaca request is answered beneath Clients::Alpaca at its Faraday adapter (decisions.rb's ScriptedAlpaca pattern),
# and any other connection raises. Re-run when a file under "ported_sources" changes (the Rust test fails until then).
require 'json'
require 'digest'

class Unscripted < Exception; end # rubocop:disable Lint/InheritException

# The test environment runs dry, which answers key_validity without a request; the hosts are what a live run reaches.
Rails.configuration.dry_run = false

Net::HTTP.prepend(Module.new { def connect = raise(Unscripted, "real connection to #{address}") })

module HostRecorder
  mattr_accessor :seen, default: []

  module Adapter
    def call(env)
      raise Unscripted, "not Alpaca: #{env.url}" unless env.url.host.to_s.end_with?('alpaca.markets')

      HostRecorder.seen << [env.method.to_s.upcase, env.url.host, env.url.path]
      body = env.url.path == '/v2/positions' || env.url.path == '/v2/account/activities' ? '[]' : '{"status":"ACTIVE","id":"x"}'
      env.response = Faraday::Response.new
      save_response(env, 200, body, { 'Content-Type' => 'application/json' })
      @app.call(env)
    end
  end
end
Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(HostRecorder::Adapter)

key_for = ->(passphrase) { ApiKey.new(key: 'PKHOSTS', secret: 'hosts-secret', passphrase:, key_type: :trading) }

# Each call names the Rails entry point; `client` is the trading client set_client builds, `data` market_data_client.
calls = {
  'account' => ->(ex) { ex.send(:client).get_account },
  'positions' => ->(ex) { ex.send(:client).get_positions },
  'order_create' => ->(ex) { ex.send(:client).create_order(symbol: 'BTC/USD', side: 'buy', type: 'market', time_in_force: 'gtc', notional: '60') },
  'order_get' => ->(ex) { ex.send(:client).get_order(order_id: 'OID') },
  'account_activities' => ->(ex) { ex.send(:client).get_account_activities },
  'clock' => ->(ex) { ex.send(:client).get_clock },
  'stock_snapshots' => ->(ex) { ex.send(:market_data_client).get_snapshots(symbols: ['AAPL']) },
  'stock_bars' => ->(ex) { ex.send(:market_data_client).get_bars(symbol: 'AAPL', timeframe: '1Day') },
  'crypto_quotes' => ->(ex) { ex.send(:market_data_client).get_crypto_latest_quote(symbols: ['BTC/USD']) },
  'crypto_trades' => ->(ex) { ex.send(:market_data_client).get_crypto_latest_trade(symbols: ['BTC/USD']) },
  'key_validity' => ->(ex) { ex.get_api_key_validity(api_key: ex.api_key) }
}

rows = []
[nil, 'paper', 'live', 'Live', 'LIVE', 'live ', ' live', ''].each do |passphrase|
  calls.each do |name, call|
    ex = Exchanges::Alpaca.new(name: 'Alpaca')
    ex.set_client(api_key: key_for.(passphrase))
    HostRecorder.seen = []
    call.(ex)
    raise "#{name}: expected one request, got #{HostRecorder.seen.inspect}" unless HostRecorder.seen.size == 1

    method, host, path = HostRecorder.seen.first
    rows << { 'passphrase' => passphrase, 'call' => name, 'method' => method, 'path' => path, 'host' => host }
  end
end
# No key at all: Exchanges::Alpaca#paper_mode?(nil).
ex = Exchanges::Alpaca.new(name: 'Alpaca')
ex.set_client(api_key: nil)
HostRecorder.seen = []
ex.send(:client).get_account
rows << { 'passphrase' => nil, 'call' => 'no_key_account', 'method' => 'GET', 'path' => '/v2/account', 'host' => HostRecorder.seen.first[1] }

exchanges_src = File.read('app/models/exchanges/alpaca.rb')
paper_mode = exchanges_src[/^  def paper_mode\?\(api_key\)\n.*?^  end\n/m] or raise 'Exchanges::Alpaca#paper_mode? not found'
out = {
  'rows' => rows,
  'ported_sources' => {
    'app/models/clients/alpaca.rb' => Digest::SHA256.file('app/models/clients/alpaca.rb').hexdigest,
    'app/models/exchanges/alpaca.rb' => Digest::SHA256.hexdigest(exchanges_src)
  }
}
File.write(ARGV.fetch(0), "#{JSON.pretty_generate(out)}\n")
puts "#{rows.size} rows"
