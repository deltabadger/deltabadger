# Long fixture payloads and source manifests are kept literal for byte inspection.
# rubocop:disable Layout/LineLength
# MCP wire recorder. All databases and outputs are below the caller's scratch root.
OAUTH_PARITY = true
load Rails.root.join('script/rust/pages.rb')
require 'base64'
require 'webmock'
WebMock.enable!
WebMock.disable_net_connect!
module McpParity
  extend ActiveSupport::Testing::TimeHelpers

  module_function

  NAMES = %w[list_bots list_exchanges list_transactions list_tax_jurisdictions].freeze
  AT = Pages::AT
  HEADERS = %w[content-type www-authenticate mcp-session-id mcp-protocol-version allow cache-control x-frame-options x-xss-protection
               x-content-type-options x-permitted-cross-domain-policies referrer-policy content-security-policy-report-only].freeze
  def rpc(method, params = {}, id = 1, extra = {})
    { 'method' => 'POST', 'path' => '/mcp', 'body' => { 'jsonrpc' => '2.0', 'id' => id, 'method' => method, 'params' => params }.compact.to_json,
      'headers' => { 'Authorization' => 'Bearer m2-token', 'Content-Type' => 'application/json', 'Accept' => 'application/json, text/event-stream' }.merge(extra) }
  end

  def init(extra = {}) = rpc('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'm2', version: '1' } }, 1, extra)
  def session_rpc(method, params = {}, id = 2, extra = {}) = rpc(method, params, id, { 'Mcp-Session-Id' => '$session' }.merge(extra))
  def ready = [init, session_rpc('notifications/initialized', {}, nil)]
  def tool(name, args = {}) = session_rpc('tools/call', { name:, arguments: args })

  def scenarios
    s = {
      'handshake' => [init, session_rpc('ping'), session_rpc('tools/list'), session_rpc('notifications/initialized', {}, nil),
                      session_rpc('tools/list'), session_rpc('notifications/initialized', {}, nil)],
      'reads' => ready + NAMES.map { |n| tool(n) },
      'unknown' => ready + [tool('unknown'), tool('market_buy'), tool('create_bot')],
      'no_token' => [init('Authorization' => nil)],
      'bad_token' => [init('Authorization' => 'Bearer unknown')],
      'scope' => [init('Authorization' => 'Bearer api-token')],
      'revoked' => [init('Authorization' => 'Bearer revoked-token')],
      'expired' => [init('Authorization' => 'Bearer expired-token')],
      'orphan' => [init('Authorization' => 'Bearer orphan-token')],
      'no_accept' => [init('Accept' => nil)],
      'accept_wildcard' => [init('Accept' => '*/*')],
      'accept_q' => [init('Accept' => 'application/json;q=1, text/event-stream;q=0')],
      'origin' => [init('Origin' => 'https://evil.example')],
      'origin_local' => [init('Origin' => 'https://localhost:7890')],
      'content_type' => [init('Content-Type' => 'text/plain')],
      'parse' => [init.merge('body' => '{')],
      'batch' => [init.merge('body' => '[]')],
      'scalar' => [init.merge('body' => '42')],
      'null_id' => [init.merge('body' => '{"jsonrpc":"2.0","id":null,"method":"ping"}')],
      'array_params' => [init.merge('body' => '{"jsonrpc":"2.0","id":1,"method":"ping","params":[]}')],
      'missing_session' => [rpc('ping')],
      'unknown_session' => [session_rpc('ping', {}, 2, 'Mcp-Session-Id' => 'no-such-session')],
      'init_session' => ready + [init('Mcp-Session-Id' => '$session')],
      'init_unknown_session' => [init('Mcp-Session-Id' => 'no-such-session')],
      'protocol' => ready + [session_rpc('ping', {}, 2, 'MCP-Protocol-Version' => '2025-06-18')],
      'methods' => ready + [session_rpc('ping'), session_rpc('unknown'), session_rpc('prompts/list'), session_rpc('resources/list'),
                            session_rpc('tasks/list'), session_rpc('logging/setLevel', { level: 'debug' }), session_rpc('completion/complete', { argument: { name: 'x', value: '' }, ref: { type: 'ref/prompt', name: 'absent' } })],
      'notification' => ready + [session_rpc('notifications/bad', {}, nil), session_rpc('notifications/cancelled', { requestId: 2 }, nil),
                                 session_rpc('notifications/progress', { progressToken: 2, progress: 1 }, nil)],
      'params' => [rpc('initialize'),
                   rpc('initialize',
                       { protocolVersion: 1, capabilities: {},
                         clientInfo: {} })] + ready + [session_rpc('tools/call', {}), session_rpc('tools/list', { cursor: 4 }),
                                                       session_rpc('tools/call', { name: 'list_bots', arguments: [] }), session_rpc('logging/setLevel', { level: 'bogus' })],
      'tool_params' => ready + [tool('list_bots', { status: 4 }), tool('list_bots', { extra: true }), tool('list_transactions', { limit: '2' }),
                                tool('list_transactions', { bot_id: nil }), tool('list_transactions', { extra: 2, limit: '2' })],
      'transactions' => ready + [{}, { limit: 1 }, { limit: 0 }, { limit: -1 }, { limit: 1.9 }, { limit: 200 }, { bot_id: 1 }, { bot_id: 1.8 },
                                 { bot_id: 999 }, { bot_id: 2 }].map { |a| tool('list_transactions', a) },
      'bots' => ready + %w[scheduled stopped archived deleted nonsense 2].map { |v| tool('list_bots', { status: v }) },
      'cursor' => ready + ['', '!', 'eA', 'MA', 'MQ', 'OTk5'].map { |v| session_rpc('tools/list', { cursor: v }) },
      'ungranted' => ready + [tool('list_bots').merge('sql' => ["UPDATE connected_clients SET mcp_tools = '[]'"]), session_rpc('tools/list')],
      'disabled' => ready + [tool('list_bots').merge('sql' => [%q(UPDATE users SET mcp_settings = '{"tool_permissions":{"list_bots":false}}')]),
                             session_rpc('tools/list')],
      'oversize' => ready + [session_rpc('ping', { filler: 'x' * 65_536 })],
      'get' => [init.merge('method' => 'GET', 'body' => '')],
      'delete' => ready + [session_rpc('ping').merge('method' => 'DELETE', 'body' => ''), session_rpc('ping'),
                           session_rpc('ping').merge('method' => 'DELETE', 'body' => '')],
      'delete_missing' => [rpc('ping').merge('method' => 'DELETE', 'body' => '')],
      'health' => [rpc('ping').merge('method' => 'GET', 'path' => '/mcp/up', 'body' => '')],
      'challenge_prefix' => [rpc('ping', {}, 1, 'Authorization' => nil).merge('path' => '/mcpanything')],
      'repeated' => ready + Array.new(25) { session_rpc('ping') }
    }
    %w[http://localhost/ http://localhost?q=1 http://u@localhost http://localhost#f ftp://localhost null HTTPS://LOCALHOST http://[::1]].each_with_index { |v, i| s["origin_edge_#{i}"] = [init('Origin' => v)] }
    ['Bearer', 'bearer   ', 'Basic x', 'bEaReR m2-token', "Bearer\tm2-token"].each_with_index do |v, i|
      s["bearer_edge_#{i}"] = [init('Authorization' => v)]
    end
    s['order_challenge'] =
      [init('Authorization' => nil, 'Origin' => 'https://evil.example', 'Content-Type' => 'text/plain', 'Accept' => nil).merge('body' => '{')]
    s['order_origin'] =
      [init('Authorization' => 'Bearer invalid', 'Origin' => 'https://evil.example', 'Content-Type' => 'text/plain', 'Accept' => nil)]
    s['order_accept'] = [init('Authorization' => 'Bearer invalid', 'Accept' => nil)]
    s['order_params'] = [rpc('tools/call', {})]
    s['client_response'] = ready + [rpc('ping').merge('headers' => session_rpc('ping')['headers'], 'body' => '{"jsonrpc":"2.0","id":7,"result":{}}')]
    s['old_negotiated'] =
      ready + [session_rpc('ping').merge('sql' => ["UPDATE action_mcp_sessions SET protocol_version='2025-06-18'"]),
               session_rpc('ping', {}, 2, 'MCP-Protocol-Version' => '2025-11-25')]
    s['get_existing'] =
      ready + [session_rpc('ping').merge('method' => 'GET', 'body' => ''),
               session_rpc('ping', {}, 2, 'MCP-Protocol-Version' => 'old').merge('method' => 'GET', 'body' => '')]
    s['empty_reads'] =
      ready + [tool('list_transactions').merge('sql' => ['DELETE FROM transactions']), tool('list_bots').merge('sql' => ['DELETE FROM bots']),
               tool('list_exchanges').merge('sql' => ['DELETE FROM api_keys'])]
    s['extra_types'] =
      ready + [tool('list_bots').merge('sql' => ["UPDATE bots SET type='Bots::DcaIndex' WHERE id=3",
                                                 %q(UPDATE bots SET type='Bots::DcaSingleAsset', settings='{"base_asset_id":2,"quote_asset_id":1,"quote_amount":10,"interval":"day"}' WHERE id=1)])]
    s['missing_assets'] =
      ready + [tool('list_bots').merge('sql' => [%q(UPDATE bots SET settings='{"allocations":{"999":1},"quote_asset_id":999}' WHERE id=1)])]
    s['no_grant_row'] = ready + [session_rpc('tools/list').merge('sql' => ['DELETE FROM connected_clients']), tool('list_bots')]
    s['unknown_stored_grant'] =
      ready + [session_rpc('tools/list').merge('sql' => [%q(UPDATE connected_clients SET mcp_tools='["unknown","list_bots","list_bots"]')]),
               tool('market_buy')]
    s['all_names_absent'] = ready + [session_rpc('tools/list')] + (AppConfig::MCP_TOOL_DEFAULTS.keys - NAMES).map { |name| tool(name) }
    s['cross_user'] =
      ready + [tool('list_bots').merge('headers' => tool('list_bots')['headers'].merge('Authorization' => 'Bearer second-token')),
               session_rpc('ping').merge('method' => 'DELETE', 'body' => '',
                                         'headers' => session_rpc('ping')['headers'].merge('Authorization' => 'Bearer second-token'))]
    s['personal_owner'] =
      ready + [
        tool('list_bots').merge('sql' => ['UPDATE oauth_applications SET personal_access_token=1,personal_owner_id=1',
                                          'DELETE FROM connected_clients']), tool('list_bots').merge('sql' => ['UPDATE oauth_applications SET personal_owner_id=2'])
      ]
    s['health_html'] = [rpc('ping', {}, 1, 'Accept' => 'text/html').merge('method' => 'GET', 'path' => '/mcp/up', 'body' => '')]
    s['health_default'] = [rpc('ping', {}, 1, 'Accept' => nil).merge('method' => 'GET', 'path' => '/mcp/up', 'body' => '')]
    s['health_head'] = [rpc('ping').merge('method' => 'HEAD', 'path' => '/mcp/up', 'body' => '')]
    s['health_unchecked_token'] = [rpc('ping', {}, 1, 'Authorization' => 'Bearer unknown').merge('method' => 'GET', 'path' => '/mcp/up', 'body' => '')]
    s['initialize_failure'] =
      [init.merge('sql' => ["CREATE TRIGGER m2_fail BEFORE INSERT ON action_mcp_session_messages BEGIN SELECT RAISE(ABORT,'synthetic failure'); END"])]
    s['tool_exception'] = ready + [tool('list_bots').merge('sql' => ["UPDATE bots SET type='NotAClass' WHERE id=1"])]
    s['progress'] = ready + [session_rpc('tools/list', { _meta: { progressToken: 0 } }), session_rpc('tools/wrong')]
    s['transactions_null_side'] =
      ready + [tool('list_transactions', { limit: 4 }).merge('sql' => ['UPDATE transactions SET side=NULL,status=1 WHERE id=1'])]
    s['numeric_formats'] =
      ready + [tool('list_transactions',
                    { limit: 4 }).merge('sql' => ['UPDATE transactions SET amount_exec=0,price=10000000000000000,quote_amount_exec=0.10000000000000001 WHERE id=1',
                                                  'UPDATE transactions SET amount_exec=0.000000000000000123456789123456789,price=-3.141592653589793,quote_amount_exec=123456789.123456789 WHERE id=2',
                                                  "UPDATE transactions SET amount_exec=10000000000000002,price=1.23456789123456789e30,quote_amount_exec='123_456.789123456789' WHERE id=3"])]
    prefixes = ENV.fetch('MCP', '').split(',')
    s.select { |name, _| prefixes.empty? || prefixes.any? { |p| name.start_with?(p) } }
  end

  def seed(dir)
    Pages.connect(dir)
    travel_to(Time.iso8601(AT), with_usec: true) do
      u = User.new(email: 'm2@example.com', password: Pages::PASSWORD, confirmed_at: Time.current, time_zone: 'Warsaw', admin: true)
      u.save!(validate: false)
      app = Doorkeeper::Application.create!(name: 'M2', uid: 'm2-client', redirect_uri: 'http://localhost/cb', confidential: false, scopes: 'mcp api')
      { 'm2-token' => {}, 'api-token' => { scopes: 'api' }, 'revoked-token' => { revoked_at: Time.current },
        'expired-token' => { created_at: 2.hours.ago }, 'orphan-token' => { resource_owner_id: nil } }.each do |token, over|
        Doorkeeper::AccessToken.create!({ application: app, resource_owner_id: u.id, token:, scopes: 'mcp', expires_in: 3600, refresh_token: "refresh-#{token}" }.merge(over)).update_columns(
          token: token, refresh_token: "refresh-#{token}"
        )
      end
      second = User.new(email: 'm2-second@example.com', password: Pages::PASSWORD, confirmed_at: Time.current, time_zone: 'UTC')

      second.save!(validate: false)
      Doorkeeper::AccessToken.create!(application: app, resource_owner_id: second.id, scopes: 'mcp',
                                      expires_in: 3600).update_columns(token: 'second-token')
      ConnectedClient.create!(user: second, oauth_application: app, mcp_tools: NAMES)
      ConnectedClient.create!(user: u, oauth_application: app, mcp_tools: ENV['MCP_TRANSPORT_ONLY'] ? [] : NAMES)
      e = Exchanges::Alpaca.create!(name: 'Alpaca', available: true)
      usd = Asset.create!(external_id: 'm2-usd', name: 'US Dollar', symbol: 'USD', category: 'Currency')
      a = Asset.create!(external_id: 'm2-btc', name: 'Bitcoin', symbol: 'BTC', category: 'Cryptocurrency')
      b = Asset.create!(external_id: 'm2-eth', name: 'Ether', symbol: 'ETH', category: 'Cryptocurrency')
      now = Time.current
      3.times do |i|
        Bot.insert_all!([{ id: i + 1, type: 'Bots::DcaMultiAsset', user_id: u.id, exchange_id: e.id, label: "Bot #{i + 1}", status: [2, 3, 7][i],
                           settings: { quote_asset_id: usd.id, allocations: { b.id.to_s => 0.4, a.id.to_s => 0.6 }, quote_amount: '50.00', interval: 'week' }, transient_data: {}, created_at: now, updated_at: now }])
      end
      105.times do |i|
        Transaction.insert_all!([{ bot_id: (i % 3) + 1, exchange_id: e.id, side: i % 2, status: i % 3, base: 'BTC', quote: 'USD',
                                   amount_exec: i.zero? ? nil : 0.00001, price: i.zero? ? nil : 1234.5, quote_amount_exec: i.zero? ? nil : 0.012345, created_at: now - i.minutes, updated_at: now }])
      end
      ApiKey.insert_all!([{ user_id: u.id, exchange_id: e.id, key_type: 0, status: 1, created_at: now, updated_at: now }])
    end
    ActiveRecord::Base.connection_pool.disconnect!
  end

  def configure
    ActionMCP.configuration.server_instructions[0] =
      "Deltabadger is a user's personal investing server. Available exchanges: #{Exchange.tradeable.pluck(:name).join(', ')}."
    ActionMCP.configuration.authentication_methods = ['bearer_token']
    ActiveJob::Base.queue_adapter = :test
    Rack::Attack.enabled = true
    Rack::Attack.cache.store = ActiveSupport::Cache::MemoryStore.new
  end

  def play(dir, steps, session_id = nil)
    Pages.connect(dir)
    configure
    McpReads.setup(dir)
    browser = ActionDispatch::Integration::Session.new(Rails.application)
    browser.host! 'localhost:3000'
    responses = []
    travel_to(Time.iso8601(AT), with_usec: true) do
      steps.each do |step|
        Array(step['sql']).each { |sql| ActiveRecord::Base.connection.execute(sql) }
        hs = step['headers'].transform_values { |v| v == '$session' ? session_id : v }.compact
        browser.process(step['method'].downcase.to_sym, step['path'], params: step['body'], headers: hs)
        raise 'unscripted MCP market request' if defined?(ScriptedMarket) && ScriptedMarket.gaps&.any?
        r = browser.response
        session_id = r.headers['Mcp-Session-Id'] || session_id
        responses << { 'status' => r.status, 'headers' => HEADERS.to_h { |h| [h, r.headers[h]] }.compact, 'body' => r.body }
      end
    end
    result = { responses:, session: session_id, rows: snapshot }
    File.write(File.join(dir, 'rails.json'), JSON.pretty_generate(result))
    ActiveRecord::Base.connection_pool.disconnect!
    result
  end
  TABLES = %w[action_mcp_sessions action_mcp_session_messages action_mcp_session_subscriptions oauth_access_tokens connected_clients bots transactions
              api_keys].freeze
  def snapshot
    TABLES.to_h { |t| [t, ActiveRecord::Base.connection.select_all("SELECT * FROM #{t} ORDER BY id").to_a.map { |row| row.transform_values { |v| v.is_a?(Float) ? { 'float_bits' => [v].pack('G').unpack1('H*') } : v } }] }
  end

  def grid(root)
    raise 'MCP root must be empty' if Dir.exist?(root) && !Dir.empty?(root)

    FileUtils.mkdir_p(root)
    template = Pages.template(root)
    seed(template)
    raise 'empty MCP grid' if scenarios.empty?

    scenarios.each do |name, steps|
      dir = File.join(root, name)
      FileUtils.mkdir_p(dir)
      %w[production.sqlite3 production_queue.sqlite3].each { |f| FileUtils.cp(File.join(template, f), File.join(dir, f)) }
      File.write(File.join(dir, 'steps.json'), JSON.pretty_generate(steps))
    end
    puts "#{scenarios.size} MCP scenarios"
  end

  def record(root)
    Dir[File.join(root, '*/steps.json')].each { |p| play(File.dirname(p), JSON.parse(File.read(p))) }
  end

  def validation_vectors(path)
    cases = []
    ActionMCP::ProtocolValidator::REQUEST_PARAM_SCHEMAS.merge(ActionMCP::ProtocolValidator::NOTIFICATION_PARAM_SCHEMAS).each do |name, schema|
      values = [nil, true, 1, 1.5, 'x', [], {}]
      schema.fetch('properties', {}).each_key { |key| [nil, true, 1, 1.5, 'x', [], {}, { 'name' => 3 }].each { |v| values << { key => v } } }
      values.each do |v|
        errors = JSONSchemer.schema(schema).validate(v).map { |e| e['error'] }.uniq
        cases << { schema: name, value: v, errors: }
      end
    end
    File.write(path, "#{JSON.pretty_generate(cases)}\n")
  end

  def metadata(path)
    configure
    data = { 'requests' => ActionMCP::ProtocolValidator::REQUEST_PARAM_SCHEMAS, 'required_requests' => ActionMCP::ProtocolValidator::REQUIRED_REQUEST_PARAMS,
             'notifications' => ActionMCP::ProtocolValidator::NOTIFICATION_PARAM_SCHEMAS, 'required_notifications' => ActionMCP::ProtocolValidator::REQUIRED_NOTIFICATION_PARAMS,
             'tools' => (AppConfig::MCP_TOOL_DEFAULTS.keys & (NAMES + McpReads::NAMES)).map { |n| ActionMCP::ToolsRegistry.find(n).to_h(protocol_version: '2025-11-25') },
             'tax' => BotApi::Tax::ListJurisdictions.call.data,
             'sources' => %w[Gemfile.lock config/mcp.yml config/routes.rb config/initializers/rack_attack.rb config/initializers/mcp_instructions.rb config/initializers/version.rb config/initializers/action_mcp_time_zone.rb app/lib/tool_access.rb app/models/connected_client.rb app/models/app_config.rb app/models/user.rb app/models/exchange.rb app/models/api_key.rb app/models/transaction.rb app/models/bot.rb app/models/bots/dca_single_asset.rb app/models/bots/dca_multi_asset.rb app/models/bots/dca_multi_asset/allocatable.rb app/models/bots/dca_index.rb app/models/bots/signal.rb app/models/automation/labelable.rb app/models/tax/jurisdictions.rb].concat(Dir['app/mcp/**/*.rb'] + %w[app/services/bot_api/bots/list.rb app/services/bot_api/exchanges/list.rb app/services/bot_api/transactions/list.rb app/services/bot_api/tax/list_jurisdictions.rb app/services/bot_api/bots/get.rb app/services/bot_api/exchanges/balances.rb app/services/bot_api/orders/list_open.rb app/services/bot_api/orders/lookup.rb app/services/bot_api/portfolio/summary.rb app/models/bot/composition/liquidatable.rb app/models/bot/composition/redeployable.rb app/models/bot/wash_sale_guard.rb app/models/exchanges/alpaca.rb app/models/bots/dca_single_asset/measurable.rb app/models/bot/composition/measurable.rb app/models/bot/asset_configurable.rb app/models/bot/reversible.rb app/models/bot/price_limitable.rb app/models/bot/price_drop_limitable.rb app/models/bot/moving_average_limitable.rb app/models/bot/indicator_limitable.rb]).to_h do |p|
               [p, Digest::SHA256.file(Rails.root.join(p)).hexdigest]
             end }
    File.write(path, "#{JSON.pretty_generate(data)}\n")
  end
end
require_relative 'mcp_reads'
command, root = ARGV
case command
when 'reads_grid' then McpReads.grid(root)
when 'grid', 'record', 'metadata', 'validation_vectors' then McpParity.public_send(command, root)
when 'play' then McpParity.play(root, JSON.parse(File.read(File.join(root, 'leg.json'))), ARGV[2])
else raise 'grid|record|metadata|play <scratch path>'
end

# rubocop:enable Layout/LineLength
