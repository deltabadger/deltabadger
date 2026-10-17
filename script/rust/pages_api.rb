# The REST API (/api/v1, Api::V1::BaseController) on the page grid: every request is made with a bearer token from no
# browser session, and every step asserts it wrote nothing (`may_mutate: []`) and enqueued nothing (`jobs`).
# Seeded per scenario ('api' => { ... }): one OAuth client of the owner's and the second user's, the owner's personal
# application, and tokens on them:
#   api-token      owner, `api`            mcp-token       owner, `mcp` only (403)
#   both-token     owner, `mcp api`        second-token    the second user, `api` (their own data)
#   personal-token owner, on the owner's personal application (no grant needed)
#   foreign-personal-token  the second user, on the owner's personal application (a grant is needed, and there is none)
#   revoked-token, expired-token, chained-token (came from a refresh: an API call retires nothing)
# 'tools' are the REST tools the owner has on (default: every one the scenario's routes need); 'grant' is the client's
# `rest_tools` (default: the same list; nil stores NULL, which follows the owner's list).
module Pages
  module ApiGrid
    def scenarios
      super.merge(api_scenarios)
    end

    # Compared beside Pages::HEADERS on every API step.
    EXTRA = %w[etag content-disposition x-total-transactions x-returned-transactions x-truncated].freeze
    READS = { 'bots' => ['/api/v1/bots', 'list_bots'], 'exchanges' => ['/api/v1/exchanges', 'list_exchanges'],
              'transactions' => ['/api/v1/transactions', 'list_transactions'],
              'tax' => ['/api/v1/tax/jurisdictions', 'list_tax_jurisdictions'], 'indices' => ['/api/v1/indices', 'list_indices'] }.freeze
    # Routes this build answers with the 501 envelope, past the same authentication and tool gate as Rails.
    GATED = [['GET', '/api/v1/bots/1', 'get_bot_details'], ['POST', '/api/v1/bots/1/start', 'start_bot'], ['GET', '/api/v1/portfolio', 'get_portfolio_summary'],
             ['DELETE', '/api/v1/rules/1', 'delete_rule'], ['GET', '/api/v1/tax/reports/DE/2025/download', 'download_tax_report']].freeze
    TOOLS = (READS.values.map(&:last) + GATED.map(&:last) + %w[create_bot create_index_bot market_buy]).freeze

    def api(path, token = 'api-token', method: 'GET', headers: {}, body: nil, expect: nil)
      step = { 'method' => method, 'path' => path, 'headers' => (token ? { 'Authorization' => "Bearer #{token}" } : {}).merge(headers),
               'csrf' => 'none', 'client' => 'api', 'action_snapshot' => true, 'may_mutate' => [], 'jobs' => true, 'headers_extra' => EXTRA }
      step['raw_body'] = body if body
      step['expect'] = expect if expect
      step
    end

    def json_body(path, body, token = 'api-token', **opts) = api(path, token, headers: { 'Content-Type' => 'application/json' }, body:, **opts)

    def api_case(steps, api = {}, attrs = {})
      { 'user' => owner, 'install' => 'alpaca', 'venues' => true, 'extra_users' => [user('email' => 'second@example.com', 'admin' => false)],
        'bots' => [{ 'kind' => 'basket', 'columns' => { 'label' => 'Basket <&>' } }, { 'kind' => 'single' }, { 'kind' => 'index' },
                   { 'kind' => 'kraken', 'columns' => { 'status' => 2 } }],
        'api' => api, 'steps' => steps }.merge(attrs)
    end

    def api_scenarios
      grid = {}
      READS.each do |name, (path, tool)|
        grid["api_#{name}_tokens"] = api_case([api(path, nil), api(path, nil, headers: { 'Authorization' => 'Basic YTpi' }), api(path, 'nope'),
                                              api(path, 'mcp-token'), api(path), api(path, 'both-token'), api(path, 'second-token'),
                                              api(path, 'personal-token'), api(path, 'foreign-personal-token'), api(path, 'revoked-token'),
                                              api(path, 'expired-token'), api(path, 'chained-token'), api(path, " \tapi-token "),
                                              api(path, nil, headers: { 'Authorization' => 'bearer api-token' })])
        grid["api_#{name}_tool_off"] = api_case([api(path), api(path, 'personal-token'), api(path, 'mcp-token')], 'tools' => TOOLS - [tool])
        grid["api_#{name}_not_granted"] = api_case([api(path), api(path, 'personal-token')], 'grant' => TOOLS - [tool])
        grid["api_#{name}_null_grant"] = api_case([api(path), api(path, 'personal-token')], 'grant' => nil)
        grid["api_#{name}_no_client"] = api_case([api(path)], 'no_client' => true)
        grid["api_#{name}_formats"] = api_case([api("#{path}.json"), api("#{path}/"), api("#{path}.xml"),
                                               api(path, method: 'HEAD'), api(path, headers: { 'Accept' => 'text/html' })])
        # Only an action that reads `params` parses the body: Rails' 400 is theirs alone.
        malformed = json_body(path, '{"limit":')
        malformed['rails_exception_status'] = 400 if %w[bots transactions indices].include?(name)
        grid["api_#{name}_malformed_json"] = api_case([malformed])
      end
      GATED.each do |method, path, tool|
        slug = "#{method.downcase}_#{path.delete_prefix('/api/v1/').tr('/', '_')}"
        grid["api_gated_#{slug}"] = api_case([api(path, nil, method:), api(path, 'mcp-token', method:)])
        grid["api_gated_#{slug}_tool_off"] = api_case([api(path, method:)], 'tools' => TOOLS - [tool])
        # A write Rails would make is no refusal to compare: only the reads and a refused delete are listed.
        grid["not_ported_api_#{slug}"] = api_case([api(path, method:)]) unless tool == 'start_bot'
      end
      grid['api_gated_create_bot_types'] = api_case([json_body('/api/v1/bots', '{"type":"signal"}').merge('method' => 'POST'),
                                                     json_body('/api/v1/bots', '{"type":"nope"}').merge('method' => 'POST'),
                                                     api('/api/v1/bots?type=%3Cb%3E', method: 'POST')])
      grid['api_gated_create_order_types'] = api_case([api('/api/v1/orders?type=limit_buy', method: 'POST'),
                                                       json_body('/api/v1/orders', '{"type":"nope"}').merge('method' => 'POST')])
      grid['not_ported_api_create_bot'] = api_case([json_body('/api/v1/bots', '{"type":"dca"}').merge('method' => 'POST')])
      # Paths Rails does not route: a locale prefix, an unknown resource, a method a resource does not take.
      # Answered before any controller, so no snapshot: the missing_ divergence compares the whole answer.
      missing = ->(path, method = 'GET') { api_case([api(path, method:).except('action_snapshot', 'may_mutate', 'jobs')]) }
      %w[/en/api/v1/bots /de/api/v1/exchanges /api/v1/nope /api/v1 /api/v1/transactions/1 /api/v1/bots.json.x /api/v1/bots/1.2.3].each do |path|
        grid["missing_api_#{path.delete_prefix('/').gsub(/[^a-z0-9]+/, '_')}"] = missing.(path)
      end
      grid['missing_api_delete_bots'] = missing.('/api/v1/bots', 'DELETE')
      grid['missing_api_put_price'] = missing.('/api/v1/transactions/account/1/price', 'PUT')
      # Rack::ConditionalGet: a 200 GET or HEAD whose etag is exactly the If-None-Match is a 304 without a body.
      bots = '/api/v1/bots'
      grid['api_conditional_get'] = api_case([api(bots), api(bots).merge('if_none_match' => 'last'), api(bots).merge('if_none_match' => 'last_strong'),
                                              api(bots, method: 'HEAD').merge('if_none_match' => 'last'), api(bots).merge('if_none_match' => '*'),
                                              api(bots), api(bots, 'second-token').merge('if_none_match' => 'last'), api(bots),
                                              api(bots, nil).merge('if_none_match' => 'last'), api('/api/v1/tax/jurisdictions'),
                                              api('/api/v1/tax/jurisdictions').merge('if_none_match' => 'last'),
                                              json_body('/api/v1/bots', '{"type":"nope"}').merge('method' => 'POST', 'if_none_match' => 'last')])
      # The setup gate is ApplicationController's: an install without an admin still answers the API.
      grid['api_no_admin'] = api_case([api(bots), api(bots, nil)], {}, 'user' => owner('admin' => false))
      grid.merge(api_param_scenarios)
    end

    def api_param_scenarios
      bots = '/api/v1/bots'
      txns = '/api/v1/transactions'
      indices = '/api/v1/indices'
      {
        'api_bots_status' => api_case([api("#{bots}?status=stopped"), api("#{bots}?status=2"), api("#{bots}?status=scheduled"), api("#{bots}?status=nope"),
                                       api("#{bots}?status=%20"), api("#{bots}?status=stopped&status=created"),
                                       # Query over body; ParamsWrapper adds `bot` beside the keys and changes nothing read.
                                       json_body("#{bots}?status=created", '{"status":"stopped"}'), json_body(bots, '{"status":"stopped"}'),
                                       json_body(bots, '{"status":2}'), json_body(bots, '[1]'), json_body(bots, ''),
                                       # `where(status: [...])`: either status; an unknown or blank member matches nothing.
                                       api("#{bots}?status[]=stopped&status[]=created"), api("#{bots}?status[]=nope&status[]=stopped"),
                                       api("#{bots}?status[]=2"), api("#{bots}?status[]="), api("#{bots}?status[]=nope"),
                                       json_body(bots, '{"status":["stopped",1]}'), json_body(bots, '{"status":[]}')]),
        'api_bots_none' => api_case([api(bots), api('/api/v1/exchanges'), api(txns), api(indices)], {}, 'bots' => [], 'api_keys' => {}),
        'api_transactions_rows' => api_case([api(txns), api("#{txns}?bot_id=1"), api("#{txns}?bot_id=2&limit=1"), api("#{txns}?bot_id=999"),
                                             api("#{txns}?bot_id=abc"), api("#{txns}?bot_id="), api("#{txns}?bot_id=4"), api("#{txns}?limit=0"),
                                             api("#{txns}?limit=-3"), api("#{txns}?limit=2abc"), api("#{txns}?limit=500"), api("#{txns}?limit=1e2"),
                                             api("#{txns}?limit=99999999999999999999"), api("#{txns}?bot_id=1&bot_id=2"),
                                             json_body("#{txns}?bot_id=1", '{"bot_id":2,"limit":1}'), json_body(txns, '{"bot_id":2,"limit":1.9}'),
                                             json_body(txns, '{"bot_id":null,"limit":null}'), json_body(txns, '{"bot_id":"2"}'),
                                             api(txns, 'second-token')], { 'transactions' => true }),
        'api_indices_filters' => api_case([api(indices), api("#{indices}?exchange_name=alpaca"), api("#{indices}?exchange_name=KRAKEN"),
                                           api("#{indices}?exchange_name=Binance"), api("#{indices}?exchange_name=Nope%3Cx%3E"), api("#{indices}?exchange_name=%20"),
                                           json_body("#{indices}?exchange_name=Kraken", '{"exchange_name":"Nope"}')], { 'indices' => true }),
        'api_indices_coingecko' => api_case([api(indices), api("#{indices}?exchange_name=Kraken")], { 'indices' => true, 'provider' => 'coingecko' }),
        'api_indices_unconfigured' => api_case([api(indices), api("#{indices}?exchange_name=Nope")], { 'indices' => true, 'unconfigured' => true }),
        'api_exchanges_keys' => api_case([api('/api/v1/exchanges'), api('/api/v1/exchanges', 'second-token')], { 'keys' => true })
      }
    end

    def action_fixture(scenario)
      super
      spec = scenario['api'] or return

      owner = User.find_by!(email: 'owner@example.com')
      second = User.find_by!(email: 'second@example.com')
      tools = spec.fetch('tools', TOOLS)
      owner.update_columns(rest_settings: { 'tool_permissions' => tools.index_with(true) })
      second.update_columns(rest_settings: { 'tool_permissions' => TOOLS.index_with(true) })
      client = Doorkeeper::Application.create!(name: 'Client', uid: 'api-client', redirect_uri: 'http://localhost/cb', confidential: false, scopes: 'mcp api')
      personal = Doorkeeper::Application.create!(name: 'Personal API token', uid: 'personal-app', redirect_uri: 'https://localhost/personal-access-token', confidential: false, scopes: 'api',
                                                 personal_access_token: true, personal_owner_id: owner.id)
      token = lambda do |value, app, who, scopes, **columns|
        Doorkeeper::AccessToken.create!(application: app, resource_owner_id: who.id, scopes:, expires_in: columns.delete(:expires_in))
                               .tap { |row| row.update_columns({ token: value }.merge(columns)) }
      end
      token.('api-token', client, owner, 'api')
      token.('mcp-token', client, owner, 'mcp')
      token.('both-token', client, owner, 'mcp api')
      token.('second-token', client, second, 'api')
      token.('personal-token', personal, owner, 'api')
      token.('foreign-personal-token', personal, second, 'api')
      token.('revoked-token', client, owner, 'api', revoked_at: Time.iso8601('2026-09-01T00:00:00Z'))
      token.('expired-token', client, owner, 'api', expires_in: 3600, created_at: Time.iso8601('2026-09-10T10:00:00Z'))
      token.('refreshed-from', client, owner, 'api', refresh_token: 'old-refresh')
      token.('chained-token', client, owner, 'api', previous_refresh_token: 'old-refresh', refresh_token: 'new-refresh')
      unless spec['no_client']
        ConnectedClient.create!(user: owner, oauth_application: client, mcp_tools: [], rest_tools: spec.fetch('grant', tools))
                       .tap { |row| row.update_columns(rest_tools: nil) if spec.key?('grant') && spec['grant'].nil? }
      end
      ConnectedClient.create!(user: second, oauth_application: client, mcp_tools: [], rest_tools: TOOLS)
      api_transactions if spec['transactions']
      api_indices(spec) if spec['indices']
      api_keys(owner, second) if spec['keys']
    end

    # Rows with every value shape a transaction renders: decimals of each scale, nil amounts, a sell, a failure, and
    # one of the second user's that the owner never sees.
    def api_transactions
      basket, single = Bot.find(1), Bot.find(2)
      rows = [order_row(basket, 'QQQM', '2026-09-10T11:00:01.5Z', amount_exec: '0.00000001', price: '123.4500', quote_amount_exec: '1e-20'),
              order_row(basket, 'IBIT', '2026-09-09T11:00:01Z', side: 'sell', amount_exec: '100000000000000000000', price: '0', quote_amount_exec: '5'),
              order_row(single, 'QQQM', '2026-09-08T11:00:01Z', status: 'failed', price: nil),
              order_row(single, 'QQQM', '2026-09-07T11:00:01Z', status: 'skipped', amount_exec: '-1.5', price: '0.1', quote_amount_exec: '3.14159265358979'),
              order_row(Bot.find(4), 'QQQM', '2026-09-06T11:00:01Z', side: nil, amount_exec: '7', price: '2', quote_amount_exec: '14')]
      Transaction.insert_all!(rows.each_with_index.map { |row, i| row.merge(external_id: "api-#{i}") })
    end

    def api_indices(spec)
      Index.create!(external_id: 'layer-1', source: 'coingecko', name: 'Layer 1 <L1>', top_coins: %w[bitcoin ethereum solana],
                    available_exchanges: { 'Exchanges::Kraken' => %w[bitcoin], 'Exchanges::Alpaca' => %w[bitcoin], 'Exchanges::Gone' => [] })
      Index.create!(external_id: 'top-coins', source: 'internal', name: 'Top coins', top_coins: nil,
                    available_exchanges: { 'Exchanges::Binance' => [], 'Exchanges::Kraken' => [] })
      Index.where(external_id: 'nasdaq-100').update_all(available_exchanges: { 'Exchanges::Alpaca' => [] })
      Exchange.find_by!(name: 'Binance').update_columns(available: false)
      AppConfig.where(key: %w[market_data_provider market_data_url market_data_token]).delete_all if spec['unconfigured']
      AppConfig.set('market_data_provider', spec['provider']) if spec['provider']
    end

    # A key of each status (from ApiKey's own list), a withdrawal key the list leaves out, and one of the second user's.
    def api_keys(owner, second)
      ApiKey.delete_all
      statuses = ApiKey.statuses.keys
      [['Alpaca', statuses[0]], ['Kraken', statuses[2]], ['Binance', statuses[3]]].each do |name, status|
        ApiKey.create!(user: owner, exchange: Exchange.find_by!(name:), key: 'k', secret: 's', status:, key_type: :trading)
      end
      ApiKey.create!(user: owner, exchange: Exchange.find_by!(name: 'Interactive Brokers'), key: 'k', secret: 's', status: statuses[1], key_type: :withdrawal)
      ApiKey.create!(user: second, exchange: Exchange.find_by!(name: 'Kraken'), key: 'k', secret: 's', status: statuses[1], key_type: :trading)
    end
  end
  singleton_class.prepend(ApiGrid)
end
