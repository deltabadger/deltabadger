# rust/tests/mcp.rs's Rails leg, run by 2.70.21 in the published image (common::v2): McpParity.play of
# script/rust/mcp.rb at tag rails-oracle-final, without the recorder harness the production bundle cannot load
# (pages.rb, webmock; the container has no network). The install in ARGV[0] plays ARGV[0]/leg.json under the MCP
# grid's clock, starting from the session ARGV[1] (if any), and writes ARGV[0]/expected.json.
require 'json'
require 'active_support/testing/time_helpers'
include ActiveSupport::Testing::TimeHelpers

AT = '2026-09-10T12:00:30.123456Z'.freeze
HEADERS = %w[content-type www-authenticate mcp-session-id mcp-protocol-version allow cache-control x-frame-options x-xss-protection
             x-content-type-options x-permitted-cross-domain-policies referrer-policy content-security-policy-report-only].freeze
TABLES = %w[action_mcp_sessions action_mcp_session_messages action_mcp_session_subscriptions oauth_access_tokens connected_clients bots transactions
            api_keys].freeze

dir, session_id = ARGV
steps = JSON.parse(File.read(File.join(dir, 'leg.json')))
raise 'a scripted market needs mcp_reads.rb' if File.exist?(File.join(dir, 'market.json'))
raise 'a queued tax report needs pages.rb' if steps.any? { |step| step['tax_queued'] }

ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
ActionMCP.configuration.server_instructions[0] =
  "Deltabadger is a user's personal investing server. Available exchanges: #{Exchange.tradeable.pluck(:name).join(', ')}."
ActionMCP.configuration.authentication_methods = ['bearer_token']
ActiveJob::Base.queue_adapter = :test
Rack::Attack.enabled = true
Rack::Attack.cache.store = ActiveSupport::Cache::MemoryStore.new
browser = ActionDispatch::Integration::Session.new(Rails.application)
browser.host! 'localhost:3000'
responses = []
travel_to(Time.iso8601(AT), with_usec: true) do
  steps.each do |step|
    Array(step['sql']).each { |sql| ActiveRecord::Base.connection.execute(sql) }
    hs = step['headers'].transform_values { |v| v == '$session' ? session_id : v }.compact
    browser.process(step['method'].downcase.to_sym, step['path'], params: step['body'], headers: hs)
    r = browser.response
    session_id = r.headers['Mcp-Session-Id'] || session_id
    responses << { 'status' => r.status, 'headers' => HEADERS.to_h { |h| [h, r.headers[h]] }.compact, 'body' => r.body }
  end
end
rows = TABLES.to_h do |t|
  [t, ActiveRecord::Base.connection.select_all("SELECT * FROM #{t} ORDER BY id").to_a.map { |row| row.transform_values { |v| v.is_a?(Float) ? { 'float_bits' => [v].pack('G').unpack1('H*') } : v } }]
end
File.write(File.join(dir, 'expected.json'), JSON.pretty_generate(responses:, session: session_id, rows:))
ActiveRecord::Base.connection_pool.disconnect!
