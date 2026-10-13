# The Rails half of the tracker check on a copy of an instance (rust/tests/tracker_on_copy.rs is the Rust half):
# Tracker::LedgerJob on a scratch copy at a frozen time, with data-api and the ECB scripted (every price answer empty,
# the ECB and the Bundesbank unavailable), then the scopes, today's snapshot rows and the requests, as JSON.
#   bin/rails runner script/rust/tracker_on_copy.rb <copy dir> <user id> <ISO time> <out.json>
# Run from a clean export with RAILS_ENV=test, every *_DATABASE_URL pointing at scratch files, TZ=UTC. The copy is
# written to (today's rows, the cache); its `market_data_provider` setting is removed first, on both halves, so the
# provider is the scripted data-api and no encrypted setting is read. No key is decrypted.
require 'json'
require 'active_support/testing/time_helpers'

ENV['TZ'] = 'UTC'
ENV['MARKET_DATA_URL'] = 'http://data-api:3000'
ENV['MARKET_DATA_TOKEN'] = 'copy-token'
ENV['COINGECKO_API_KEY'] = ''

module CopyHarness
  class Unscripted < Exception; end # rubocop:disable Lint/InheritException

  mattr_accessor :requests

  def self.reply(env)
    host = env.url.host.to_s
    key = "#{env.method.to_s.upcase} #{env.url.path}"
    kind = if host == 'data-api' then 'market'
           elsif host == 'data-api.ecb.europa.eu' || host == 'api.statistiken.bundesbank.de' then 'ecb'
           end
    raise Unscripted, "unscripted HTTP call #{key} to #{host}" unless kind

    requests[kind] << [key, URI.decode_www_form(env.url.query.to_s).sort]
    return { 'status' => 500, 'body' => '' } if kind == 'ecb'
    raise Unscripted, "unscripted call #{key}" unless key == 'GET /api/v1/historical_prices'

    { 'status' => 200, 'body' => JSON.generate('prices' => []) }
  end

  module Adapter
    def call(env)
      reply = CopyHarness.reply(env)
      env.response = Faraday::Response.new
      save_response(env, reply['status'], reply['body'], { 'Content-Type' => 'application/json' })
      @app.call(env)
    end
  end
end

Net::HTTP.prepend(Module.new { def connect = raise(CopyHarness::Unscripted, "real connection to #{address}:#{port}") })

dir, user_id, at, out = ARGV
raise ArgumentError, 'usage: <copy dir> <user id> <ISO time> <out.json>' unless out

[:net_http_persistent, :net_http, Faraday.default_adapter].uniq.each do |name|
  Faraday::Adapter.lookup_middleware(name).prepend(CopyHarness::Adapter)
end
ActiveJob::Base.queue_adapter = :test
Rails.cache = ActiveSupport::Cache::MemoryStore.new
Turbo::StreamsChannel.singleton_class.prepend(Module.new do
  %i[broadcast_remove_to broadcast_update_to broadcast_append_to broadcast_replace_to broadcast_refresh_to].each { |m| define_method(m) { |*, **| nil } }
end)
ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
ActiveRecord::Base.connection.exec_delete("DELETE FROM app_configs WHERE key = 'market_data_provider'")
CopyHarness.requests = { 'market' => [], 'ecb' => [] }

def d(value) = value&.to_d&.to_s('F')

def summary(s)
  { 'positions' => s.positions.sort_by(&:symbol).map do |p|
      { 'symbol' => p.symbol, 'quantity' => d(p.quantity), 'cost' => d(p.cost_usd), 'avg_cost' => d(p.avg_cost_usd),
        'estimated' => p.estimated ? true : false, 'unpriced' => d(p.unpriced_quantity) }
    end,
    'total_invested' => d(s.total_invested_usd), 'cash' => s.cash.map { |c, a| [c, d(a)] }, 'cash_basis' => s.cash_basis.map { |c, a| [c, d(a)] },
    'incomplete' => s.incomplete ? true : false, 'loss_sales' => s.loss_sales.map { |sym, on| [sym, on.to_s] } }
end

def raw(value) = value.is_a?(Float) ? { 'f' => [value].pack('G').unpack1('H*') } : value

user = User.find(user_id)
time = Time.iso8601(at)
error = nil
scopes = nil
extend ActiveSupport::Testing::TimeHelpers
travel_to(time) do
  Tracker::LedgerJob.perform_now(user.id)
  scopes = Rails.cache.read(Tracker::Ledger.send(:cache_key, user))
rescue StandardError => e
  error = "#{e.class}: #{e.message}"
end
today = time.utc.to_date.to_s
tables = { 'portfolio_snapshots' => 'date', 'portfolio_venue_snapshots' => 'exchange_id, date' }.to_h do |table, order|
  rows = ActiveRecord::Base.connection.select_all("SELECT * FROM #{table} WHERE date = '#{today}' ORDER BY #{order}").to_a
  [table, rows.map { |r| r.except('id', 'created_at', 'updated_at').transform_values { |v| raw(v) } }]
end
report = { 'error' => error, 'tables' => tables, 'requests' => CopyHarness.requests,
           'whole' => scopes && summary(scopes[nil]), 'venues' => scopes && scopes.except(nil).sort.to_h { |id, s| [id.to_s, summary(s)] } }
File.write(out, JSON.pretty_generate(report))
