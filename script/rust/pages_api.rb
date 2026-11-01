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
              'tax' => ['/api/v1/tax/jurisdictions', 'list_tax_jurisdictions'], 'indices' => ['/api/v1/indices', 'list_indices'],
              # Slice C: the reads that may ask a venue (Alpaca, stubbed: WebMock here, wiremock in rust/tests/pages.rs).
              'bot' => ['/api/v1/bots/1', 'get_bot_details'], 'balances' => ['/api/v1/exchanges/1/balances', 'get_exchange_balances'],
              'orders' => ['/api/v1/orders', 'list_open_orders'], 'portfolio' => ['/api/v1/portfolio', 'get_portfolio_summary'] }.freeze
    # Routes this build answers with the 501 envelope, past the same authentication and tool gate as Rails.
    GATED = [['GET', '/api/v1/rules', 'list_rules'], ['DELETE', '/api/v1/rules/1', 'delete_rule']].freeze
    # The bot writes (slice B): the route, its tool, and the status bot 1 needs for the route to succeed.
    WRITES = { 'update' => ['PATCH', '/api/v1/bots/1', 'update_bot_settings', 0], 'start' => ['POST', '/api/v1/bots/1/start', 'start_bot', 0],
               'stop' => ['POST', '/api/v1/bots/1/stop', 'stop_bot', 1], 'archive' => ['POST', '/api/v1/bots/1/archive', 'archive_bot', 2],
               'unarchive' => ['DELETE', '/api/v1/bots/1/archive', 'unarchive_bot', 7], 'delete' => ['DELETE', '/api/v1/bots/1', 'delete_bot', 2] }.freeze
    # PLAN-A11: the tracker routes (pages_api.rb api_tracker_scenarios).
    TRACKER = { 'list' => ['GET', '/api/v1/transactions/account', 'list_account_transactions'],
                'export' => ['GET', '/api/v1/transactions/export', 'export_transactions_csv'],
                'link' => ['POST', '/api/v1/transactions/account/3/transfer_link', 'set_transfer_link'],
                'price' => ['PATCH', '/api/v1/transactions/account/10/price', 'set_transaction_price'],
                'sync' => ['POST', '/api/v1/tracker/sync', 'sync_tracker'] }.freeze
    # PLAN-A12 A12-1: the tax report routes (pages_api.rb api_tax_scenarios).
    TAX = { 'create' => ['POST', '/api/v1/tax/reports', 'generate_tax_report'],
            'status' => ['GET', '/api/v1/tax/reports/US/2025', 'get_tax_report_status'],
            'download' => ['GET', '/api/v1/tax/reports/US/2025/download', 'download_tax_report'] }.freeze
    TOOLS = (READS.values.map(&:last) + GATED.map(&:last) + WRITES.values.map { |w| w[2] } + TRACKER.values.map(&:last) + TAX.values.map(&:last) +
             %w[create_bot create_index_bot create_signal_bot market_buy]).freeze
    # What a bot write may change, and the rows compared as stored.
    WRITE_TABLES = %w[bots bot_activity_logs bot_index_assets].freeze
    WRITE_ROWS = { 'bots' => '*', 'bot_activity_logs' => '*', 'bot_index_assets' => '*' }.freeze

    def api(path, token = 'api-token', method: 'GET', headers: {}, body: nil, expect: nil)
      step = { 'method' => method, 'path' => path, 'headers' => (token ? { 'Authorization' => "Bearer #{token}" } : {}).merge(headers),
               'csrf' => 'none', 'client' => 'api', 'action_snapshot' => true, 'may_mutate' => [], 'jobs' => true, 'headers_extra' => EXTRA }
      step['raw_body'] = body if body
      step['expect'] = expect if expect
      step
    end

    def json_body(path, body, token = 'api-token', **opts) = api(path, token, headers: { 'Content-Type' => 'application/json' }, body:, **opts)

    # A step that may write a bot: the tables it may touch and their rows as stored.
    def write(method, path, token = 'api-token', body: nil, **opts)
      step = body ? json_body(path, body, token, method:, **opts) : api(path, token, method:, **opts)
      step.merge('may_mutate' => WRITE_TABLES, 'raw_rows' => WRITE_ROWS)
    end

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
        malformed['rails_exception_status'] = 400 if %w[bots transactions indices bot balances orders].include?(name)
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
      grid.merge(api_param_scenarios).merge(api_write_scenarios).merge(api_read_scenarios).merge(api_create_scenarios).merge(api_order_scenarios).merge(api_tracker_scenarios).merge(api_tax_scenarios)
    end

    # A10: POST /api/v1/bots (BotApi::Bots::Create, CreateIndex, CreateSignal). A refused create writes nothing: the
    # insert is the start's save (Bot::Lifecycle#start, Bots::Signal#start).
    def create(body, token = 'api-token', **opts) = write('POST', '/api/v1/bots', token, body: body.is_a?(String) ? body : body.to_json, **opts)

    CREATE_DCA = { type: 'dca', exchange_name: 'Alpaca', base_asset: 'AAPL', quote_asset: 'USD', quote_amount: 25, interval: 'week' }.freeze
    CREATE_INDEX = { type: 'index', exchange_name: 'Alpaca', index: 'nasdaq-100', quote_asset: 'USD', quote_amount: 40, interval: 'day' }.freeze
    CREATE_SIGNAL = { type: 'signal', exchange_name: 'Alpaca', base_asset: 'AAPL', quote_asset: 'USD' }.freeze

    def api_create_scenarios
      later = ->(days, time = '09:30:00') { "#{(Time.iso8601(AT) + days.days).to_date.iso8601}T#{time}" }
      dca = ->(extra) { CREATE_DCA.merge(extra) }
      index = ->(extra) { CREATE_INDEX.merge(extra) }
      signal = ->(extra) { CREATE_SIGNAL.merge(extra) }
      venues = { 'venues' => true }
      # ND100 offered on Alpaca with its members (Index::ExchangeAvailability), so the index cells reach their checks.
      api_case = ->(steps, api = {}, attrs = {}) { self.api_case(steps, { 'index_alpaca' => true }.merge(api), attrs) }
      grid = {
        'api_create_dca_single' => api_case.([create(CREATE_DCA)]),
        'api_create_dca_string_amount' => api_case.([create(dca.(quote_amount: '25.5', interval: 'hour'))]),
        'api_create_dca_basket_string' => api_case.([create(dca.(base_asset: nil, assets: 'AAPL:60,msft:40'))]),
        'api_create_dca_basket_array' => api_case.([create(dca.(base_asset: nil, assets: [{ symbol: 'aapl', allocation: 50 }, { symbol: 'MSFT', allocation: 50 }]))]),
        'api_create_dca_basket_three' => api_case.([create(dca.(base_asset: nil, assets: 'AAPL,MSFT,NVDA', interval: 'month'))]),
        'api_create_dca_basket_tolerance' => api_case.([create(dca.(base_asset: nil, assets: 'AAPL:60,MSFT:39.95'))]),
        'api_create_dca_legacy_pair' => api_case.([create(dca.(second_base_asset: 'MSFT', allocation: 70)), create(dca.(second_base_asset: 'NVDA'))]),
        'api_create_dca_market_cap' => api_case.([create(dca.(base_asset: nil, assets: 'BTC:90,ETH:10', weighting: 'market_cap'))]),
        'api_create_dca_labels' => api_case.([create(dca.(label: 'Mine <b>&"')), create(dca.(label: '  ')), create(dca.(label: ''))]),
        'api_create_dca_kraken' => api_case.([create(dca.(exchange_name: 'kraken', base_asset: 'btc', quote_asset: 'eur', quote_amount: 10, interval: 'day'))], {}, venues),
        # D-API-CREATE-HYPERLIQUID-LIMIT: Rails' start refuses its own API bot (limit orders left off); this build sets them.
        'fixed_api_create_dca_hyperliquid' => api_case.([create(dca.(exchange_name: 'Hyperliquid', base_asset: 'BTC', quote_asset: 'USDC', quote_amount: 20))], {},
                                                 venues.merge('hyperliquid_bots' => true)),
        'api_create_dca_paper_trading' => api_case.([create(CREATE_DCA)], { 'paper' => true }),
        'api_create_dca_wash_sale_undecided' => api_case.([create(CREATE_DCA), create(CREATE_INDEX), create(CREATE_SIGNAL)], {}, 'user' => owner('wash_sale_enabled' => nil)),
        'api_create_dca_twice' => api_case.([create(CREATE_DCA), create(CREATE_DCA)]),
        'api_create_index_default' => api_case.([create(CREATE_INDEX.except(:index))]),
        'api_create_index_nasdaq' => api_case.([create(CREATE_INDEX)]),
        'api_create_index_options' => api_case.([create(index.(num_coins: 5, allocation_flattening: 0.5, label: 'Top five'))]),
        'api_create_index_counts' => api_case.([1, 2, 12, 13, 2.5, '3'].map { |n| create(index.(num_coins: n)) }),
        'api_create_index_flattening' => api_case.([-0.1, 1, 2, 'x'].map { |f| create(index.(allocation_flattening: f)) }),
        'api_create_signal' => api_case.([create(CREATE_SIGNAL), create(signal.(label: 'Signals <b>'))]),
        # D-API-CREATE-REFUSED: a signal bot on IBKR, which Rails makes and starts and this build's engine does not run.
        'refused_api_create_signal_ibkr' => api_case.([create(signal.(exchange_name: 'Interactive Brokers', base_asset: 'QQQM'))], {},
                                                     'api_keys' => { 'alpaca' => 'correct', 'ibkr' => 'correct' }),
        'api_create_signal_kraken' => api_case.([create(signal.(exchange_name: 'Kraken', base_asset: 'BTC', quote_asset: 'EUR'))], {}, venues),
        # Every refusal of the services, in their order; none writes.
        'api_create_dca_refusals' => api_case.([
          create({ type: 'dca' }), create({ assets: 'AAPL' }), create(dca.(interval: 'year')), create(dca.(weighting: 'equal')),
          create(dca.(second_base_asset: 'MSFT', allocation: 101)), create(dca.(exchange_name: 'Nope')),
          create(dca.(exchange_name: 'Interactive Brokers')), create(dca.(quote_amount: 0)), create(dca.(quote_amount: 'abc')),
          create(dca.(quote_amount: -1)), create(dca.(base_asset: nil, assets: ',')), create(dca.(base_asset: nil, assets: 'AAPL:')),
          create(dca.(base_asset: nil, assets: 'AAPL,aapl')), create(dca.(base_asset: nil, assets: (1..101).map { |i| "S#{i}" }.join(','))),
          create(dca.(base_asset: nil, assets: [1, 2])), create(dca.(base_asset: 'DOGE')), create(dca.(quote_asset: 'EUR')),
          create(dca.(base_asset: nil, assets: 'AAPL:60,MSFT:30')), create(dca.(base_asset: nil, assets: 'AAPL:60,MSFT')),
          create(dca.(base_asset: nil, assets: 'AAPL,MSFT', weighting: 'market_cap')), create(dca.(base_asset: nil, assets: 'AAPL:120,MSFT:-20'))
        ]),
        # The start refuses after the service is satisfied: Rails' valid?(:start) on the unsaved bot.
        'api_create_dca_start_refused' => api_case.([
          create(dca.(base_asset: nil, assets: 'AAPL,MSFT')).merge('before' => 'untradeable_msft'),
          create(dca.(base_asset: nil, assets: 'AAPL,MSFT', start_at: '')),
          create(dca.(start_at: '2026-09-01T10:00:00Z')), create(dca.(start_at: 'nope')), create(dca.(start_at: 2)),
          # Numbered entries are permitted as a hash, which Array() makes pairs of: refused, never the base_asset bot.
          create(dca.(assets: { '0' => { symbol: 'MSFT' } }))
        ]),
        'api_create_index_refusals' => api_case.([
          create({ type: 'index' }), create(index.(interval: 'fortnight')), create(index.(exchange_name: 'Nope')),
          create(index.(exchange_name: 'Interactive Brokers')), create(index.(index: 'nope')), create(index.(quote_amount: 'x')),
          create(index.(quote_asset: 'EUR')), create(index.(start_at: ''))
        ], {}, venues),
        'api_create_index_no_market_data' => api_case.([create(CREATE_INDEX)], {}, 'market_data' => 'none'),
        'api_create_signal_refusals' => api_case.([
          create({ type: 'signal' }), create(signal.(exchange_name: 'Nope')), create(signal.(exchange_name: 'Interactive Brokers')),
          create(signal.(base_asset: 'DOGE')), create(signal.(base_asset: 'MSFT')).merge('before' => 'untradeable_msft')
        ]),
        'api_create_gates' => api_case.([create(CREATE_DCA, nil), create(CREATE_DCA, 'mcp-token'), create(CREATE_DCA, 'second-token'),
                                        create('{"type":"dca",').merge('rails_exception_status' => 400)]),
        'api_create_type_query' => api_case.([write('POST', '/api/v1/bots?type=signal&exchange_name=Alpaca&base_asset=AAPL&quote_asset=USD')]),
        'api_create_tool_off' => api_case.([create(CREATE_DCA), create(CREATE_INDEX), create(CREATE_SIGNAL)], 'tools' => TOOLS - %w[create_bot create_index_bot create_signal_bot]),
        'api_create_not_granted' => api_case.([create(CREATE_DCA), create(CREATE_INDEX), create(CREATE_SIGNAL)], 'grant' => TOOLS - %w[create_bot create_index_bot create_signal_bot]),
        'api_create_null_grant' => api_case.([create(CREATE_SIGNAL)], 'grant' => nil)
      }
      # start_at as Rails' schedule_start_at reads it: in the account's zone (Tallinn) unless it names its own offset. A wall time
      # the zone repeats (2026-10-25T03:30) is D-API-CREATE-START-AT-AMBIGUOUS, tested in rust/tests/api_create.rs: Rails takes
      # the earlier one; this build refuses it as the web's start-time form does.
      { 'z' => "#{later.(3)}Z", 'offset' => "#{later.(3)}+02:00", 'naive' => later.(3), 'date' => later.(3)[0, 10],
        'dst_gap' => '2027-03-28T03:30', 'offset_compact' => "#{later.(3)}+0200", 'offset_no_seconds' => "#{later.(3)[0, 16]}+02:00",
        'z_no_seconds' => "#{later.(3)[0, 16]}Z" }.each do |name, at|
        grid["api_create_start_at_#{name}"] = api_case.([create(dca.(start_at: at)), create(index.(start_at: at))])
      end
      grid
    end

    # Bot 1 in the status a route needs (one that ran has a start time); bots 2 to 4 as api_case has them.
    def write_case(steps, status, api = {}, attrs = {}, bot = {})
      columns = { 'label' => 'Basket <&>', 'status' => status }
      columns['started_at'] = '2026-09-09T12:00:00Z' unless status.zero?
      api_case(steps, api, attrs).tap do |c|
        c['bots'] = [{ 'kind' => 'basket', 'columns' => columns }.merge(bot)] + c['bots'].drop(1)
      end
    end

    def api_write_scenarios
      grid = {}
      WRITES.each do |name, (method, path, tool, status)|
        body = name == 'update' ? '{"quote_amount":60}' : nil
        ok = ->(token = 'api-token') { write(method, path, token, body:) }
        # Every refusal first (no writes), then the one token that acts.
        grid["api_#{name}_tokens"] = write_case([ok.(nil), ok.('nope'), ok.('mcp-token'), ok.('second-token'), ok.('foreign-personal-token'),
                                                 ok.('revoked-token'), ok.('expired-token'), ok.('personal-token')], status)
        grid["api_#{name}_tool_off"] = write_case([ok.(), ok.('personal-token')], status, 'tools' => TOOLS - [tool])
        grid["api_#{name}_not_granted"] = write_case([ok.(), ok.('personal-token')], status, 'grant' => TOOLS - [tool])
        grid["api_#{name}_null_grant"] = write_case([ok.()], status, 'grant' => nil)
        # Rails' params: the path's id wins over the query's and the body's; an id Rails' to_i reads as another.
        grid["api_#{name}_ids"] = write_case([write(method, "#{path.sub('/1', '/999')}?id=1", body: '{"id":1,"quote_amount":60}'),
                                              write(method, path.sub('/1', '/abc')), write(method, path.sub('/1', '/0')),
                                              write(method, path.sub('/1', '/1abc'), body:),
                                              write(method, "#{path}?id=2", body: '{"id":3,"quote_amount":60}')], status)
        grid["api_#{name}_malformed_json"] = write_case([write(method, path, body: '{"quote_amount":').merge('rails_exception_status' => 400)], status)
      end
      grid.merge(api_update_scenarios, api_lifecycle_scenarios, api_edge_scenarios)
    end

    # The path's id as the router hands it over (percent-decoded, `+` kept, a bad escape or byte refused), and
    # parameters past this build's per-request bounds.
    def api_edge_scenarios
      theirs = { 'kind' => 'basket', 'owner' => 'second@example.com', 'columns' => { 'status' => 1, 'started_at' => '2026-09-09T12:00:00Z' } }
      many = (1..1001).map { |i| "\"S#{i}\":1" }.join(',')
      streams = { 'broadcasts' => %w[user_1:bot_updates user_1:bot_1] }
      {
        # The owner's bots page is open while an API client acts: it hears what the web's own actions send.
        'api_broadcasts' => write_case(signed_in(get('/bots')) + [write('POST', '/api/v1/bots/1/start').merge(streams), write('POST', '/api/v1/bots/1/stop').merge(streams),
                                                                 write('POST', '/api/v1/bots/1/archive').merge(streams), write('DELETE', '/api/v1/bots/1/archive').merge(streams),
                                                                 write('PATCH', '/api/v1/bots/1', body: '{"quote_amount":70}').merge(streams),
                                                                 write('DELETE', '/api/v1/bots/1').merge(streams)], 2),
        # Rails renders a bot's broadcasts in its owner's locale (Bot#with_user_locale); the API's answers stay English.
        'api_broadcasts_de' => write_case(signed_in(get('/bots')) + [write('POST', '/api/v1/bots/1/start').merge(streams), write('POST', '/api/v1/bots/1/stop').merge(streams),
                                                                    write('POST', '/api/v1/bots/1/archive').merge(streams), write('DELETE', '/api/v1/bots/1/archive').merge(streams)],
                                          2, {}, 'user' => owner('locale' => 'de')),
        # The same from the owner's own bot page under /de/: what the open pages hear is in the owner's language (Polish) and
        # under /pl/ (Bot#with_user_locale); the page's answers stay German.
        'api_broadcasts_web_owner_locale' => write_case(signed_in(get('/de/bots/1')) + [
          ['PATCH', '/de/bots/1/start?start_fresh=true'], ['PATCH', '/de/bots/1/stop'], ['POST', '/de/bots/1/archive'],
          ['DELETE', '/de/bots/1/archive'], ['DELETE', '/de/bots/1/delete']
        ].map { |method, path|
          { 'method' => method, 'path' => path, 'form' => {}, 'csrf' => 'header', 'headers' => TURBO.merge('Referer' => 'http://localhost:3000/de/bots/1'),
            'may_mutate' => WRITE_TABLES, 'raw_rows' => WRITE_ROWS }.merge(streams)
        }, 2, {}, 'user' => owner('locale' => 'pl')),
        'api_stop_encoded_ids' => write_case([write('POST', '/api/v1/bots/1%30/stop'), write('POST', '/api/v1/bots/%35/stop'),
                                              write('POST', '/api/v1/bots/%E2%82/stop'),
                                              write('POST', '/api/v1/bots/%35/stop', 'second-token'), write('POST', '/api/v1/bots/1+/stop'),
                                              write('POST', '/api/v1/bots/%31/stop')], 1).tap { |c| c['bots'] << theirs },
        'api_start_oversized' => write_case([write('POST', '/api/v1/bots/1/start', body: "{\"start_fresh\":[#{(['true'] * 1001).join(',')}]}")], 2),
        'api_update_oversized' => write_case([write('PATCH', '/api/v1/bots/1', body: "{\"allocations\":{#{many}}}"),
                                              write('PATCH', '/api/v1/bots/1', body: "{\"label\":[#{(['1'] * 1001).join(',')}],\"quote_amount\":61}")], 2)
      }
    end

    def api_update_scenarios
      bot = '/api/v1/bots/1'
      upd = ->(body, path = bot, method: 'PATCH') { write(method, path, body:) }
      {
        'api_update_values' => write_case([upd.('{"quote_amount":"75.5","interval":"week","label":"New <b>&"}'),
                                           write('PUT', "#{bot}?quote_amount=80&smart_intervaled=false"),
                                           write('PATCH', "#{bot}?smart_intervaled=true&smart_interval_quote_amount=7"),
                                           upd.('{"allocations":{"QQQM":70,"IBIT":30}}'), upd.('{"allocations":"qqqm:50,IBIT:50"}'),
                                           upd.('{"limit_ordered":false,"label":null}'), upd.('{"quote_amount":60.0}')], 2),
        'api_update_refusals' => write_case([upd.(''), upd.('{}'), upd.('{"nope":1}'), upd.('{"quote_amount":"abc"}'), upd.('{"quote_amount":-5}'),
                                             upd.('{"quote_amount":"1e3"}'), upd.('{"num_coins":5}'),
                                             # Overlapping causes: the catalogue's order picks the refusal.
                                             upd.('{"num_coins":5,"quote_amount":"abc"}'), upd.('{"quote_amount":"abc","num_coins":5}'),
                                             upd.('{"allocations":"NOPE:50"}'), upd.('{"allocations":"QQQM:50,qqqm:50"}'), upd.('{"allocations":[1]}'),
                                             upd.('{"allocations":"QQQM:150"}'), upd.('{"label":["a"],"quote_amount":{"a":1}}'),
                                             upd.('{"quote_amount":0.5}'), upd.('{"interval":"fortnight"}'), upd.('{"quote_amount":"abc","label":""}'),
                                             upd.('{"label":"   "}'), upd.('{"quote_amount":null}')], 2),
        'api_update_running' => write_case([upd.('{"quote_amount":"abc"}'), upd.('{}'), upd.('{"quote_amount":60}')], 1),
        # Rails resyncs an index bot's composition in Bot::ResyncIndexCompositionJob where an index-definition key changed;
        # this build leaves the engine its resync marker there (engine::index::RESYNC_KEY), compared as that job.
        'api_update_index' => write_case([write('PATCH', '/api/v1/bots/3', body: '{"num_coins":5,"allocation_flattening":"0.5"}'),
                                          write('PATCH', '/api/v1/bots/3', body: '{"allocation_flattening":2}'),
                                          write('PATCH', '/api/v1/bots/3', body: '{"allocations":"QQQM:50"}'),
                                          write('PATCH', '/api/v1/bots/3', body: '{"num_coins":2.5}'),
                                          write('PATCH', '/api/v1/bots/3?num_coins=7')], 2),
        'api_update_others' => write_case([write('PATCH', '/api/v1/bots/1', 'second-token', body: '{"quote_amount":60}'),
                                           write('PATCH', '/api/v1/bots/999', body: '{"quote_amount":60}'),
                                           write('PATCH', '/api/v1/bots/4', body: '{"quote_amount":60}'),
                                           write('PATCH', '/api/v1/bots/1', body: '{"quote_amount":60}')], 7)
      }
    end

    def api_lifecycle_scenarios
      bot = '/api/v1/bots/1'
      start = ->(query = '', token = 'api-token', body: nil) { write('POST', "#{bot}/start#{query}", token, body:) }
      owed = { 'user' => owner('wash_sale_enabled' => nil) }
      selling = { 'settings' => { 'rebalance_enabled' => true } }
      {
        'api_start_flags' => write_case([start.('?start_fresh=junk'), start.('?start_fresh=false'), start.(), start.('?start_fresh=junk'),
                                         start.('?start_fresh=true')], 2),
        'api_start_flag_values' => write_case([start.('', body: '{"start_fresh":"yes"}'), start.('', body: '{"start_fresh":[true]}'),
                                               start.('?start_fresh=', body: nil), start.('', body: '{"start_fresh":0}')], 2),
        'api_start_fresh_json' => write_case([start.('', body: '{"start_fresh":true}')], 2),
        'api_start_continue_json' => write_case([start.('?start_fresh=1', body: '{"start_fresh":false}')], 0),
        # Rails asks the wash-sale question first, then reads the flag, then whether the bot runs.
        'api_start_wash_sale' => write_case([start.('?start_fresh=junk'), start.()], 1, {}, owed, selling),
        'api_start_wash_sale_buyer' => write_case([start.()], 0, {}, owed),
        'api_start_invalid' => write_case([start.(), start.('?start_fresh=junk')], 0, {}, {}, 'stored' => { 'quote_amount' => 0 }),
        'api_start_coins' => write_case([start.()], 0, {}, {}, { 'kind' => 'coins' }),
        # Named divergence: Rails starts a bot on a stock listing no sync has typed (instrument_type nil); this build's
        # guard refuses it (engine::eligibility, "only plain cryptocurrencies, and stocks and ETFs on Alpaca"), writing nothing.
        'refused_api_start_untyped' => write_case([start.()], 0, { 'untyped' => true }),
        'api_start_archived' => write_case([start.()], 7),
        'api_start_no_key' => write_case([start.()], 0, {}, 'api_keys' => {}),
        'api_start_others' => write_case([start.('', 'second-token'), write('POST', '/api/v1/bots/999/start'), write('POST', '/api/v1/bots/4/start')], 0),
        # PLAN-wash-sale WS-3: the engine runs the bot of an account with the rule on (or not yet answered); its Stop always
        # passes, as Rails' BotApi::Bots::Stop never asks the setting.
        'api_stop_wash_rule_on' => write_case([write('POST', "#{bot}/stop")], 1, {}, 'user' => owner('wash_sale_enabled' => true)),
        'api_stop_wash_question' => write_case([write('POST', "#{bot}/stop")], 1, {}, 'user' => owner('wash_sale_enabled' => nil)),
        'api_stop_states' => write_case([write('POST', "#{bot}/stop"), write('POST', "#{bot}/stop"), write('POST', '/api/v1/bots/3/stop'),
                                         write('POST', '/api/v1/bots/1/stop', 'second-token')], 4),
        'api_archive_states' => write_case([write('POST', "#{bot}/archive"), write('POST', "#{bot}/archive"), write('DELETE', "#{bot}/archive"),
                                            write('DELETE', "#{bot}/archive"), write('POST', '/api/v1/bots/3/archive')], 1),
        'api_delete_states' => write_case([write('DELETE', bot), write('DELETE', bot), write('POST', "#{bot}/start"), write('DELETE', '/api/v1/bots/4'),
                                           write('DELETE', '/api/v1/bots/3', 'second-token')], 1)
      }
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

    # Slice C's reads past the gate: each service's lookups and the venue's answers (paper and live hosts, a failing
    # venue). Every step still writes nothing. A bot with fills is left out: Rails prices it from data.alpaca.markets,
    # which this build's reads do not ask (their figures come from the walk alone).
    def api_read_scenarios
      bot = '/api/v1/bots'
      balances = ->(id) { "/api/v1/exchanges/#{id}/balances" }
      orders = '/api/v1/orders'
      {
        'api_bot_lookups' => api_case([api("#{bot}/1"), api("#{bot}/2"), api("#{bot}/3"), api("#{bot}/4"), api("#{bot}/999"), api("#{bot}/abc"),
                                       api("#{bot}/2abc"), api("#{bot}/1?id=2"), api("#{bot}/1", 'second-token'), api("#{bot}/1", 'personal-token')]),
        'api_bot_started' => api_case([api("#{bot}/1")]).tap { |c| c['bots'][0]['columns'] = { 'label' => 'Basket <&>', 'status' => 1, 'started_at' => '2026-09-09T12:00:00Z' } },
        'api_balances_lookups' => api_case([api(balances.(1)), api(balances.(2)), api(balances.(999)), api(balances.('abc')), api(balances.('1abc')),
                                            api(balances.(1), 'second-token'), api("#{balances.(1)}?id=2")]),
        'api_balances_held' => api_case([api(balances.(1)), api("#{balances.(1)}.json")], { 'alpaca_key' => 'PKRICH' }),
        'api_balances_live' => api_case([api(balances.(1)), api(orders)], { 'alpaca_key' => 'PKRICH', 'passphrase' => 'live' }),
        'api_balances_venue_down' => api_case([api(balances.(1)), api(orders)], { 'alpaca_key' => 'failure-key' }),
        'api_balances_positions_refused' => api_case([api(balances.(1))], { 'alpaca_key' => 'positions-bad' }),
        'api_orders_rows' => api_case([api(orders), api("#{orders}?exchange_name=alpaca"), api("#{orders}?exchange_name=Kraken"),
                                       api("#{orders}?exchange_name=Nope%3Cx%3E"), api("#{orders}?exchange_name=%20"), api(orders, 'second-token')],
                                      { 'alpaca_key' => 'PKRICH' }).tap { |c| c['bots'][0]['orders'] = 'open' },
        'api_orders_no_key' => api_case([api(orders), api("#{orders}?exchange_name=Alpaca")], {}, 'api_keys' => {}),
        'api_portfolio_none' => api_case([api('/api/v1/portfolio'), api("#{bot}/1")], {}, 'bots' => [])
      }
    end

    # PLAN-A9 slice O1: OrdersController#create and #destroy up to the venue. Bot 1 is a basket with an open order and an
    # imported one; bots 2 to 5 are signal bots: working on QQQM, stopped, working with an unresolved placement, and working
    # on IBIT, which the venue no longer trades. A create claims its Idempotency-Key, so it may write that table, compared
    # as stored (the fingerprint holds ParamsWrapper's `order` copy of the body).
    ORDER_TOOLS = %w[market_sell limit_buy limit_sell cancel_order].freeze

    def order_bots
      open = { 'external_status' => 'open', 'order_type' => 'limit_order', 'price' => '408.25', 'amount' => '0.12247', 'amount_exec' => '0',
               'quote_amount' => nil, 'quote_amount_exec' => '0' }
      rows = [['QQQM', '2026-09-10T11:00:00Z', open.merge('external_id' => 'cancel-ok')], ['QQQM', '2026-09-10T11:01:00Z', open.merge('external_id' => 'imported_cancel-1')]]
      running = { 'status' => 1, 'started_at' => '2026-09-09T12:00:00Z' }
      marker = { 'signal_placement_unresolved' => { 'id' => 'marker-1', 'at' => '2026-09-10T11:00:00Z', 'side' => 'buy', 'error' => 'timeout' } }
      [{ 'kind' => 'basket', 'orders' => rows }, { 'kind' => 'signal', 'columns' => running }, { 'kind' => 'signal', 'columns' => { 'status' => 2 } },
       { 'kind' => 'signal', 'pair' => %w[IBIT USD], 'columns' => running, 'transient' => marker },
       { 'kind' => 'signal', 'pair' => %w[IBIT USD], 'columns' => running, 'delist' => 'IBIT' }]
    end

    def order_case(steps, api = {}, attrs = {}) = api_case(steps, { 'tools' => TOOLS + ORDER_TOOLS }.merge(api), { 'bots' => order_bots }.merge(attrs))

    def order_post(body, key, token = 'api-token')
      headers = { 'Content-Type' => 'application/json' }
      headers['Idempotency-Key'] = key if key
      api('/api/v1/orders', token, method: 'POST', headers:, body:).merge('may_mutate' => %w[idempotency_keys], 'raw_rows' => { 'idempotency_keys' => '*' })
    end

    def api_order_scenarios
      buy = '{"type":"market_buy","exchange_name":"Alpaca","base_asset":"QQQM","quote_asset":"USD","amount":10}'
      refusals = ['{"type":"market_buy"}',
                  '{"type":"limit_sell","exchange_name":"Alpaca","base_asset":"QQQM","quote_asset":"USD","amount":"1","price":" "}',
                  '{"type":"market_buy","exchange_name":"Nope<x>","base_asset":"QQQM","quote_asset":"USD","amount":10}',
                  '{"type":"market_buy","exchange_name":"interactive brokers","base_asset":"QQQM","quote_asset":"USD","amount":10}',
                  '{"type":"market_sell","exchange_name":"alpaca","base_asset":"qqqm","quote_asset":"eur","amount":10}',
                  '{"type":"market_sell","exchange_name":"Alpaca","base_asset":["QQQM"],"quote_asset":"USD","amount":10}',
                  '{"type":"market_sell","exchange_name":"ALPACA","base_asset":"ibit","quote_asset":"usd","amount":1,"amount_type":"base"}',
                  '{"type":"market_buy","bot_id":"abc","amount":10}', '{"type":"market_buy","bot_id":12.5,"amount":10}',
                  '{"type":"market_buy","bot_id":"","amount":10}', '{"type":"market_buy","bot_id":[2],"amount":10}',
                  '{"type":"market_buy","bot_id":2,"amount":"0"}', '{"type":"market_buy","bot_id":2,"amount":"1e3"}',
                  '{"type":"market_sell","bot_id":2,"amount":10,"amount_type":"QUOTE"}', '{"type":"market_buy","bot_id":999,"amount":10}',
                  '{"type":"market_buy","bot_id":1,"amount":10}', '{"type":"market_buy","bot_id":"3","amount":10}',
                  '{"type":"market_sell","bot_id":4.0,"amount":10}', '{"type":"market_buy","bot_id":5,"amount":10}',
                  '{"type":"market_buy","bot_id":2,"amount":10,"exchange_name":"alpaca","base_asset":"IBIT","quote_asset":"usd"}',
                  '{"type":"limit_buy","bot_id":2}', '{"type":"limit_sell","bot_id":null,"exchange_name":"Alpaca","amount":1}']
      cancel = ->(path, token = 'api-token') { api(path, token, method: 'DELETE') }
      {
        'api_orders_refusals' => order_case(refusals.each_with_index.map { |body, i| order_post(body, "key-#{i}") } +
                                            [order_post(buy, nil), order_post(refusals[2], 'key-2'), order_post(buy, 'key-2'),
                                             order_post('{"type":"market_buy","bot_id":2,"amount":10}', 'second-key', 'second-token'),
                                             order_post(refusals[0], 'query-wrapper').merge('path' => '/api/v1/orders?order=1')]),
        'api_orders_create_tool_off' => order_case([order_post(buy.sub('market_buy', 'market_sell'), 'off'), cancel.('/api/v1/orders/2')],
                                            'tools' => TOOLS + ORDER_TOOLS - %w[market_sell cancel_order]),
        'api_orders_wash_locked' => order_case([order_post(buy, 'wash-1'), order_post('{"type":"market_buy","bot_id":2,"amount":10}', 'wash-2')], {},
                                               'user' => owner('wash_sale_enabled' => true, 'wash_sale_jurisdiction' => 'US'),
                                               'wash_locks' => [['QQQM', '2026-09-25T00:00:00Z', 'order']]),
        'api_orders_key_pending' => order_case([order_post(buy, 'pending-1'), order_post('{"type":"market_buy","bot_id":2,"amount":10}', 'pending-2')], {},
                                               'api_keys' => { 'alpaca' => 'pending_activation' }),
        'api_orders_key_incorrect' => order_case([order_post('{"type":"market_sell","bot_id":2,"amount":10}', 'incorrect-1')], {},
                                                 'api_keys' => { 'alpaca' => 'incorrect' }),
        'api_orders_cancel_refusals' => order_case([cancel.('/api/v1/orders/abc'), cancel.('/api/v1/orders/abc?exchange_name=Nope%3Cx%3E'),
                                                    cancel.('/api/v1/orders/abc?exchange_name=Interactive%20Brokers'), cancel.('/api/v1/orders/2'),
                                                    cancel.('/api/v1/orders/2.json'), cancel.('/api/v1/orders/999'), cancel.('/api/v1/orders/abc?exchange_name=')]),
        # Past every refusal: Rails asks the venue (stubbed below), this build answers 501 until the engine places (PLAN-A9 O3, O4).
        'not_ported_api_orders_market_buy' => order_case([order_post(buy, 'placed')]),
        'not_ported_api_orders_limit_sell' => order_case([order_post('{"type":"limit_sell","exchange_name":"Alpaca","base_asset":"QQQM","quote_asset":"USD","amount":1,"price":400}', 'limit')]),
        # PLAN-A9 O2: the cancels, sent (the venue answers as pages_settings.rb stubs it): a bot's open row by its id, and order
        # ids on the exchange path (the bot row's own id among them), accepted, refused and with a key the venue rejects.
        'api_orders_cancel_row' => order_case([cancel.('/api/v1/orders/1')]),
        'api_orders_cancel_external' => order_case([cancel.('/api/v1/orders/cancel-ok?exchange_name=alpaca'), cancel.('/api/v1/orders/cancel-refused?exchange_name=Alpaca'),
                                                    cancel.('/api/v1/orders/cancel-key?exchange_name=Alpaca')])
      }
    end

    # The Alpaca answers the read scenarios stub, by key (`alpaca_key`), after pages_settings.rb's own (WebMock answers
    # the last stub declared first). The same answers are mounted on rust/tests/pages.rs's wiremock server.
    HELD_ACCOUNT = '{"status":"ACTIVE","cash":"1234.5","buying_power":"2000"}'
    HELD_POSITIONS = '[{"symbol":"QQQM","asset_class":"us_equity","qty":"2.5","qty_available":"2"}]'
    OPEN_ORDERS = '[{"id":"venue-1","symbol":"QQQM","asset_class":"us_equity","side":"buy","type":"limit","status":"new","qty":"1.5","filled_qty":"0",' \
                  '"filled_avg_price":null,"limit_price":"401.5","notional":null},' \
                  '{"id":"venue-2","symbol":"IBIT","asset_class":"us_equity","side":"sell","type":"market","status":"partially_filled","qty":"3",' \
                  '"filled_qty":"1","filled_avg_price":"55.25","limit_price":null,"notional":null},' \
                  '{"id":"order-1-QQQM-2026-09-10T11:00:00Z","symbol":"QQQM","asset_class":"us_equity","side":"buy","type":"limit","status":"new",' \
                  '"qty":"0.12247","filled_qty":"0","filled_avg_price":null,"limit_price":"408.25","notional":null},' \
                  '{"id":"venue-3","symbol":"BTC/USD","asset_class":"crypto","side":"buy","type":"market","status":"new","qty":null,"filled_qty":"0",' \
                  '"filled_avg_price":null,"limit_price":null,"notional":"10"}]'

    # ponytail: declared again at every scenario's connect (harmless repeats), so they always follow pages_settings.rb's.
    def connect(dir)
      super
      json = { 'Content-Type' => 'application/json' }
      %w[paper-api.alpaca.markets api.alpaca.markets].each do |host|
        WebMock.stub_request(:get, "https://#{host}/v2/orders?limit=50&status=open").to_return(status: 200, body: '[]', headers: json)
        WebMock.stub_request(:get, "https://#{host}/v2/orders?limit=50&status=open").with(headers: { 'APCA-API-KEY-ID' => 'PKRICH' }).to_return(status: 200, body: OPEN_ORDERS, headers: json)
        WebMock.stub_request(:get, "https://#{host}/v2/orders?limit=50&status=open").with(headers: { 'APCA-API-KEY-ID' => 'failure-key' })
               .to_return(status: 503, body: '{"message":"unavailable"}', headers: json)
        WebMock.stub_request(:get, "https://#{host}/v2/account").with(headers: { 'APCA-API-KEY-ID' => 'PKRICH' }).to_return(status: 200, body: HELD_ACCOUNT, headers: json)
        WebMock.stub_request(:get, "https://#{host}/v2/positions").with(headers: { 'APCA-API-KEY-ID' => 'PKRICH' }).to_return(status: 200, body: HELD_POSITIONS, headers: json)
      end
      # PLAN-A9: an order Rails sends past every refusal (the not_ported_api_orders_ scenarios) is refused by the venue.
      %w[paper-api.alpaca.markets api.alpaca.markets].each do |host|
        WebMock.stub_request(:post, "https://#{host}/v2/orders").to_return(status: 403, body: '{"message":"no orders in the grid"}', headers: json)
      end
      WebMock.stub_request(:get, 'https://api.alpaca.markets/v2/account').with { |request| request.headers['Apca-Api-Key-Id'] != 'PKRICH' }
             .to_return(status: 200, body: '{"status":"ACTIVE","cash":"0"}', headers: json)
      WebMock.stub_request(:get, 'https://api.alpaca.markets/v2/positions').with { |request| request.headers['Apca-Api-Key-Id'] != 'PKRICH' }
             .to_return(status: 200, body: '[]', headers: json)
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
      Index.where(external_id: 'nasdaq-100').find_each { |ix| ix.update_columns(available_exchanges: { 'Exchanges::Alpaca' => ix.top_coins }) } if spec['index_alpaca']
      api_keys(owner, second) if spec['keys']
      api_ledger(owner, second, spec['ledger']) if spec['ledger']
      api_tracker_keys(owner, spec['tracker_keys']) if spec['tracker_keys']
      # Paper trading on: MCP refuses the create tools; REST is live and creates.
      owner.update_columns(mcp_settings: { 'dry_run' => true }) if spec['paper']
      # The owner's Alpaca key as another account (its stubbed answers) or in live mode (the live host).
      if spec['alpaca_key']
        ApiKey.find_by!(user: owner, exchange: Exchange.find_by!(name: 'Alpaca')).update_columns(key: spec['alpaca_key'], passphrase: spec.fetch('passphrase', 'paper'))
      end
      # MarketData's listings sync types every stock listing (CATEGORY_BY_TYPE); the grid's alpaca install leaves them untyped.
      unless spec['untyped']
        Asset.where(category: 'Stock').find_each { |asset| asset.update_columns(instrument_type: %w[QQQM IBIT].include?(asset.symbol) ? 'etf' : 'stock') }
      end
    end

    # PLAN-A11: the tracker through REST. A write step may change account_transactions only; every step's job classes are
    # compared (sync's by key: tests/pages.rs tracker_sync_jobs). `api_tracker_d_*` scenarios hold the named divergences:
    # tests/pages.rs tracker_api_difference turns Rails' answer into the one this build owes, after checking Rails still does
    # what the divergence names.
    def tracker_write(method, path, body = nil, token = 'api-token', **opts)
      step = body ? json_body(path, body, token, method:, **opts) : api(path, token, method:, **opts)
      step.merge('may_mutate' => ['account_transactions'])
    end

    def ledger_case(steps, ledger = 'rows', api = {}, attrs = {}) = api_case(steps, { 'ledger' => ledger }.merge(api), attrs)

    def api_tracker_scenarios
      grid = {}
      TRACKER.each do |name, (method, path, tool)|
        step = ->(token = 'api-token') { %w[link price sync].include?(name) ? tracker_write(method, path, name == 'link' ? '{"linked":true}' : (name == 'price' ? '{"price_usd":"2"}' : nil), token) : api(path, token, method:) }
        grid["api_tracker_#{name}_tokens"] = ledger_case([step.(nil), step.('nope'), step.('mcp-token'), step.('revoked-token'), step.('second-token'), step.('personal-token')])
        grid["api_tracker_#{name}_tool_off"] = ledger_case([step.(), step.('personal-token')], 'rows', 'tools' => TOOLS - [tool])
        grid["api_tracker_#{name}_not_granted"] = ledger_case([step.()], 'rows', 'grant' => TOOLS - [tool])
      end
      list = '/api/v1/transactions/account'
      export = '/api/v1/transactions/export'
      link = ->(id) { "#{list}/#{id}/transfer_link" }
      price = ->(id) { "#{list}/#{id}/price" }
      sync = '/api/v1/tracker/sync'
      grid['api_tracker_list'] = ledger_case([api(list), api("#{list}?exchange_id=3"), api("#{list}?exchange_id=1abc"), api("#{list}?exchange_id=abc"),
                                              api("#{list}?exchange_id=999"), api("#{list}?exchange_id="), api("#{list}?from_date=2026-09-02"),
                                              api("#{list}?to_date=2026-09-03"), api("#{list}?from_date=2026-09-02&to_date=2026-09-05&exchange_id=1"),
                                              api("#{list}?from_date=2026-09-05&to_date=2026-09-02"), api("#{list}?from_date=invalid"),
                                              api("#{list}?to_date=2024-13-01"), api("#{list}?from_date=2024-02-30"), api("#{list}?exchange_id=999&from_date=invalid"),
                                              api("#{list}?from_date=%20"), api("#{list}?entry_type=deposit"), api("#{list}?entry_type=withdrawal&limit=2"),
                                              api("#{list}?entry_type=staking_reward"), api("#{list}?entry_type="), api("#{list}?limit=1"), api("#{list}?limit=0"),
                                              api("#{list}?limit=-3"), api("#{list}?limit=500"), api("#{list}?limit=abc"), api("#{list}?limit=2abc"),
                                              json_body(list, '{"limit":2,"entry_type":"buy"}'), json_body("#{list}?limit=1", '{"limit":3}'),
                                              json_body(list, '{"limit":null,"exchange_id":null}'), json_body(list, '{"exchange_id":3.7}'),
                                              api(list, 'second-token'), api("#{list}.json"), api(list, method: 'HEAD')])
      grid['api_tracker_list_many'] = ledger_case([api(list), api("#{list}?limit=200"), api("#{list}?limit=201")], 'many')
      # D-API-DATE-STRICT: Date.parse reads a date out of these; this build answers step 0's invalid_date.
      grid['api_tracker_d_dates'] = ledger_case([api("#{list}?from_date=invalid"), api("#{list}?from_date=Sep%205"), api("#{list}?from_date=2026-09-02T10:00"),
                                                 api("#{list}?to_date=20260903"), api("#{list}?from_date=1582-12-31"), api("#{list}?from_date=2026-09-02x"),
                                                 api("#{export}?from_date=Sep%202%202026")])
      # D-API-ENTRY-TYPE: Rails filters by any text (an empty list, an enum number, a list); this build answers 422 invalid_entry_type.
      grid['api_tracker_d_entry_type'] = ledger_case([api("#{list}?from_date=invalid"), api("#{list}?entry_type=bogus"), api("#{list}?entry_type=4"),
                                                      api("#{list}?entry_type[]=deposit&entry_type[]=withdrawal")])
      # D-API-NONSCALAR: Rails raises on Array#to_i; this build answers as step 0 (exchange) and step 1 (the default limit).
      grid['api_tracker_d_nonscalar'] = ledger_case([api("#{list}?exchange_id=999"), api(list), api("#{list}?exchange_id[]=1").merge('rails_exception_status' => 500),
                                                     api("#{list}?limit[]=1").merge('rails_exception_status' => 500),
                                                     api("#{export}?exchange_id[]=1").merge('rails_exception_status' => 500)])
      grid['api_tracker_export'] = ledger_case([api(export), api("#{export}?exchange_id=3"), api("#{export}?from_date=2026-09-02&to_date=2026-09-05"),
                                                api("#{export}?exchange_id=999"), api("#{export}?from_date=invalid"), api("#{export}?from_date=2030-01-01"),
                                                api("#{export}.json"), api(export, method: 'HEAD'), api(export).merge('if_none_match' => 'last'),
                                                api(export, 'second-token'), api("#{export}?entry_type=deposit&limit=1")])
      grid['api_tracker_export_none'] = ledger_case([api(export)], 'none')
      grid['api_tracker_export_cap'] = ledger_case([api(export)], 'cap_5000')
      grid['api_tracker_export_over_cap'] = ledger_case([api(export), api("#{export}?from_date=2026-01-01")], 'cap_5001')
      grid['api_tracker_link'] = ledger_case([tracker_write('POST', link.(3)), tracker_write('POST', link.(3), '{"linked":"maybe"}'),
                                              tracker_write('POST', link.(3), '{"linked":1}'), tracker_write('POST', "#{link.(3)}?linked="),
                                              tracker_write('POST', link.(999), '{"linked":true}'), tracker_write('POST', link.(14), '{"linked":true}'),
                                              tracker_write('POST', link.(3), '{"linked":true}'), tracker_write('POST', link.(4), '{"linked":"true"}'),
                                              tracker_write('POST', "#{link.(4)}?linked=false"), tracker_write('POST', link.(3), '{"linked":false}'),
                                              tracker_write('POST', "#{link.(4)}?linked=true"), tracker_write('POST', link.(5), '{"linked":true}'),
                                              tracker_write('POST', link.(10), '{"linked":true}'), tracker_write('POST', link.(9), '{"linked":true}'),
                                              tracker_write('POST', '/api/v1/transactions/account/8abc/transfer_link', '{"linked":false}'),
                                              tracker_write('POST', link.(6), '{"linked":false}'), tracker_write('POST', link.(3), '{"linked":true}', 'second-token')])
      grid['api_tracker_link_deposit_side'] = ledger_case([tracker_write('POST', link.(4), '{"linked":true}'), tracker_write('POST', link.(7), '{"linked":true}')])
      # D-LINK-KIND: Rails answers a buy and an airdrop with "No matching transfer"; this build with 422 not_transferable (step 0's headers).
      grid['api_tracker_d_link_kind'] = ledger_case([tracker_write('POST', link.(3), '{"linked":"maybe"}'), tracker_write('POST', link.(1), '{"linked":true}'),
                                                     tracker_write('POST', link.(2), '{"linked":true}'), tracker_write('POST', link.(2), '{"linked":false}')])
      grid['api_tracker_price'] = ledger_case([tracker_write('PATCH', price.(10)), tracker_write('PATCH', price.(10), '{"price":"2"}'),
                                               tracker_write('PATCH', "#{price.(10)}?price_usd=12.5"), tracker_write('PATCH', price.(10), '{"price_usd":"0"}'),
                                               tracker_write('PATCH', price.(10), '{"price_usd":1.2345678901234567}'), tracker_write('PATCH', price.(10), '{"price_usd":5}'),
                                               tracker_write('PATCH', price.(10), '{"price_usd":0.00005}'), tracker_write('PATCH', price.(10), '{"price_usd":" 7.50 "}'),
                                               tracker_write('PATCH', price.(10), '{"price_usd":"-5"}'), tracker_write('PATCH', price.(10), '{"price_usd":"1e3"}'),
                                               tracker_write('PATCH', price.(10), '{"price_usd":"1234567890123456"}'),
                                               tracker_write('PATCH', price.(10), '{"price_usd":"0.1234567890123456789"}'),
                                               tracker_write('PATCH', price.(10), '{"price_usd":"abc"}'), tracker_write('PATCH', price.(10), '{"price_usd":"7.50"}'),
                                               tracker_write('PATCH', price.(10), '{"price_usd":"7.5"}'), tracker_write('PATCH', price.(10), '{"price_usd":null}'),
                                               tracker_write('PATCH', price.(10), '{"price_usd":null}'), tracker_write('PATCH', price.(1), '{"price_usd":"12"}'),
                                               tracker_write('PATCH', price.(11), '{"price_usd":"12"}'), tracker_write('PATCH', price.(1), '{"price_usd":""}'),
                                               tracker_write('PATCH', price.(1), '{"price_usd":"abc"}'), tracker_write('PATCH', price.(2), '{"price_usd":"  "}'),
                                               tracker_write('PATCH', price.(999), '{"price_usd":"1"}'), tracker_write('PATCH', price.(14), '{"price_usd":"1"}'),
                                               tracker_write('PATCH', price.(10), '{"price_usd":"1"}', 'second-token'),
                                               tracker_write('PATCH', "#{price.(10)}?price_usd", nil)])
      # D-PRICE-NONTEXT: Rails clears on false, [] and {} (a row with no stated price, so nothing is written); this build answers step 0.
      grid['api_tracker_d_price_nontext'] = ledger_case([tracker_write('PATCH', price.(10), '{"price_usd":"abc"}'), tracker_write('PATCH', price.(10), '{"price_usd":false}'),
                                                         tracker_write('PATCH', price.(10), '{"price_usd":[]}'), tracker_write('PATCH', price.(10), '{"price_usd":{}}')])
      grid['api_tracker_sync'] = ledger_case([tracker_write('POST', sync), tracker_write('POST', sync, '{"x":'), tracker_write('POST', sync, nil, 'second-token')])
      grid['api_tracker_sync_none'] = ledger_case([tracker_write('POST', sync)], 'rows', {}, 'api_keys' => {})
      grid['api_tracker_sync_withdrawal'] = ledger_case([tracker_write('POST', sync)], 'rows', { 'tracker_keys' => 'withdrawal' }, 'api_keys' => {})
      grid['api_tracker_sync_two'] = ledger_case([tracker_write('POST', sync)], 'rows', 'tracker_keys' => 'two')
      grid['api_tracker_sync_fallback'] = ledger_case([tracker_write('POST', sync)], 'rows', 'tracker_keys' => 'fallback')
      # D-SYNC-VENUE: a reading key on Interactive Brokers, whose ledger this build does not read: 501, nothing woken.
      grid['api_tracker_d_sync_venue'] = ledger_case([tracker_write('POST', sync)], 'rows', {}, 'api_keys' => { 'alpaca' => 'correct', 'ibkr' => 'correct' })
      grid
    end

    # PLAN-A12 A12-1: POST /api/v1/tax/reports, GET …/:country/:year and …/download (BotApi::Tax::*). Every step lists the
    # files under the instance's tmp/tax_reports after it (`tax_files`; AppPaths.tmp is <scenario>/tmp on both sides), and a
    # create may write the owner's pending_report (users) and, on this build, its request row (rust_tax_reports; Rails: the
    # queue). Report and refusal files are placed at grid build time (`tax_fixtures`, by file name); a report already being
    # generated is the `tax_generating` step hook (Rails: an unfinished Solid Queue job under the concurrency key).
    # The grid's Rails runs ActiveJob's test adapter, so an accepted create holds no slot there: no scenario reads the status
    # of, or creates again over, a report it accepted (this build's request row would answer `generating`).
    # `api_tax_d_*` hold D-TAX-NOT-PORTED-GATE: tests/pages.rs tax_api_difference turns Rails' 202 into the 501 this build owes.
    TAX_REPORT = 'Date,Type,Asset,Amount,Proceeds (USD)' + "\n" + '2025-03-01,sell,BTC,0.5,"30,000.00"' + "\n" + '2025-04-02,sell,ETH,1,Café' + "\n"
    TAX_TOKENIZED = '{"reason":"tokenized_unsupported","symbols":["TSLAX","NVDAX"]}'
    TAX_EXCESS = '{"reason":"excess_return_of_capital","symbols":["ARB"]}'

    def tax_step(step) = step.merge('tax_files' => true)
    def tax_create(body, token = 'api-token', **opts)
      step = body.nil? ? api(TAX['create'][1], token, method: 'POST', **opts) : json_body(TAX['create'][1], body.is_a?(String) ? body : body.to_json, token, method: 'POST', **opts)
      tax_step(step).merge('may_mutate' => %w[users rust_tax_reports])
    end
    def tax_query(query, token = 'api-token', **opts) = tax_step(api("#{TAX['create'][1]}?#{query}", token, method: 'POST', **opts)).merge('may_mutate' => %w[users rust_tax_reports])
    def tax_status(country = 'US', year = 2025, token = 'api-token', **opts) = tax_step(api("/api/v1/tax/reports/#{country}/#{year}", token, **opts))
    def tax_download(country = 'US', year = 2025, token = 'api-token', **opts) = tax_step(api("/api/v1/tax/reports/#{country}/#{year}/download", token, **opts))
    def tax_case(steps, files = {}, api = {}, attrs = {}) = api_case(steps, api, { 'tax_fixtures' => files }.merge(attrs))

    def api_tax_scenarios
      us = { country: 'US', year: 2025 }
      ready = { '1_US_2025_crypto.csv' => TAX_REPORT }
      refused = { '1_US_2025_crypto.csv.refused.json' => TAX_TOKENIZED }
      generating = { 'before' => 'tax_generating' }
      grid = {}
      TAX.each do |name, (method, path, tool)|
        step = lambda do |token = 'api-token'|
          case name
          when 'create' then tax_create(us, token)
          when 'status' then tax_status('US', 2025, token)
          else tax_download('US', 2025, token)
          end
        end
        tokens = [nil, 'nope', 'mcp-token', 'revoked-token', 'expired-token', 'foreign-personal-token']
        # A create acts once per account: every refusal first, then the second user's, then the owner's (one accepted each).
        tokens += name == 'create' ? %w[second-token personal-token] : ['api-token', 'both-token', 'second-token', 'personal-token', 'chained-token', " \tapi-token "]
        grid["api_tax_#{name}_tokens"] = tax_case(tokens.map { |token| step.(token) } + [tax_step(api(path, nil, method:, headers: { 'Authorization' => 'bearer api-token' }))], ready)
        grid["api_tax_#{name}_tool_off"] = tax_case([step.(), step.('personal-token'), step.('mcp-token')], ready, 'tools' => TOOLS - [tool])
        grid["api_tax_#{name}_not_granted"] = tax_case([step.(), step.('personal-token')], ready, 'grant' => TOOLS - [tool])
      end
      # Params.resolve, then the flags, then market data: every refusal in Rails' order, none writes.
      grid['api_tax_create_refusals'] = tax_case([
        tax_create({}, expect: 422), tax_create({ country: 'US' }), tax_create({ year: 2025 }), tax_create({ country: ' ', year: 2025 }),
        tax_create({ country: 'US', year: '' }), tax_create({ country: 'US', year: ' ' }), tax_create({ country: 'US', year: 'abc' }),
        tax_create({ country: 'US', year: 2008 }), tax_create({ country: 'US', year: 2101 }), tax_create({ country: 'US', year: 2025.5 }),
        tax_create({ country: 'US', year: '2025.5' }), tax_create({ country: 'US', year: '1e3' }), tax_create({ country: 'US', year: -2025 }),
        tax_create({ country: 'US', year: { a: 1 } }), tax_create({ country: 'XX', year: 'abc' }), tax_create({ country: 'XX', year: 2025 }),
        tax_create({ country: 'usa', year: 2025 }), tax_create({ country: ['US'], year: 2025 }), tax_create({ country: 'U<b>', year: 2025 }),
        tax_create(us.merge(force: 'yes')), tax_create(us.merge(force: 1)), tax_create(us.merge(force: 'TRUE')),
        tax_create(us.merge(stablecoin_as_fiat: 'maybe')), tax_create(us.merge(stablecoin_as_fiat: 0)),
        tax_create(us.merge(force: 'yes', stablecoin_as_fiat: 'maybe')), tax_create({ country: 'XX', year: 2025, force: 'yes' }),
        tax_query('country=US&year=abc'), tax_query('country=US&year=2025&force=1'), tax_query('year=2025'),
        tax_create('{"country":"US",').merge('rails_exception_status' => 400)
      ])
      grid.merge!(
        'api_tax_create_accepted' => tax_case([tax_create({ country: 'us', year: '2025' }, expect: 202)]),
        'api_tax_create_flags' => tax_case([tax_create({ country: ' Us ', year: 2025.0, stablecoin_as_fiat: 'true', force: 'false' }, expect: 202)]),
        'api_tax_create_flags_json' => tax_case([tax_create(us.merge(stablecoin_as_fiat: true, force: true), expect: 202)]),
        'api_tax_create_flags_blank' => tax_case([tax_create(us.merge(stablecoin_as_fiat: '', force: nil), expect: 202)]),
        'api_tax_create_query' => tax_case([tax_query('country=us&year=2024&stablecoin_as_fiat=false', expect: 202)]),
        # Query over body (ParamsWrapper's `tax_report` copy changes nothing read).
        'api_tax_create_query_and_body' => tax_case([tax_step(json_body("#{TAX['create'][1]}?year=2024", '{"country":"US","year":2023}', method: 'POST'))
                                                       .merge('may_mutate' => %w[users rust_tax_reports], 'expect' => 202)]),
        # The tracker's other pending report is replaced, the rest of tracker_settings kept.
        'api_tax_create_pending_kept' => tax_case([tax_create(us, expect: 202)], {}, {},
                                                  'user' => owner('tracker_settings' => { 'scope' => 'crypto', 'pending_report' => { 'country' => 'DE', 'year' => 2024, 'report_scope' => 'broker' } })),
        'api_tax_create_second_user' => tax_case([tax_create(us, 'second-token', expect: 202)], ready),
        'api_tax_create_market_data_off' => tax_case([tax_create(us, expect: 422), tax_create({ country: 'US', year: 'abc' }), tax_status, tax_download], {}, {}, 'market_data' => 'none'),
        # A finished report answers 409 until force replaces it (the earlier one is removed once the new run is accepted).
        'api_tax_create_ready' => tax_case([tax_create(us, expect: 409), tax_create(us.merge(force: 'false')), tax_status, tax_download,
                                            tax_status('US', 2024), tax_create(us.merge(force: 'true'), expect: 202), tax_download], ready),
        'api_tax_create_ready_force_json' => tax_case([tax_create(us.merge(force: true), expect: 202)], ready),
        # A report and a refusal side by side (a refusal written after the report's run): ready wins, both step aside on force.
        'api_tax_create_ready_and_refused' => tax_case([tax_status, tax_download, tax_create(us), tax_create(us.merge(force: true), expect: 202)], ready.merge(refused)),
        # A refusal steps aside for a new run, and is gone once it is accepted.
        'api_tax_create_refused' => tax_case([tax_status, tax_download, tax_create(us, expect: 202)], refused),
        'api_tax_create_refused_other_year' => tax_case([tax_create(us.merge(year: 2024), expect: 202), tax_status], refused),
        # A run already queued or running for the account (crypto scope), for every country and year.
        'api_tax_create_generating' => tax_case([tax_status.merge(generating), tax_create(us, expect: 409), tax_create(us.merge(force: true)),
                                                 tax_create(us.merge(year: 2024)), tax_status('US', 2024), tax_download, tax_status('US', 2025, 'second-token'),
                                                 tax_create(us, 'second-token', expect: 202)]),
        'api_tax_create_generating_ready' => tax_case([tax_create(us).merge(generating), tax_create(us.merge(force: true)), tax_status, tax_download], ready),
        'api_tax_create_generating_refused' => tax_case([tax_status.merge(generating), tax_create(us), tax_download], refused),
        # Validation still answers first while a run is held.
        'api_tax_create_generating_refusals' => tax_case([tax_create({ country: 'US', year: 'abc' }).merge(generating), tax_create({ country: 'XX', year: 2025 }),
                                                          tax_create(us.merge(force: 'yes'))]),
        'api_tax_status' => tax_case([tax_status, tax_status('us', 2025), tax_status('US', 2024), tax_status('US', 2023), tax_status('US', 2022),
                                      tax_status('US', 2021), tax_status('DE', 2025), tax_status('XX', 2025), tax_status('xx', 'abc'), tax_status('US', 'abc'),
                                      tax_status('US', 2008), tax_status('US', 2101), tax_status('US', '2025.0'), tax_status('US', '2025abc'),
                                      tax_status('US', 2024, 'second-token'), tax_status('US', 2024, 'personal-token'),
                                      tax_status('US', 2024).merge('if_none_match' => 'last'), tax_status('US', 2024, method: 'HEAD'),
                                      tax_step(api('/api/v1/tax/reports/US/2024.json')), tax_status('U%20S', 2025), tax_status('%55S', 2024)],
                                     { '1_US_2024_crypto.csv' => TAX_REPORT, '1_US_2023_crypto.csv.refused.json' => TAX_EXCESS,
                                       '1_US_2022_crypto.csv.refused.json' => '{"reason":', '1_US_2021_crypto.csv.refused.json' => TAX_TOKENIZED,
                                       '1_US_2024_broker.csv' => TAX_REPORT, '2_US_2025_crypto.csv' => TAX_REPORT }),
        'api_tax_download' => tax_case([tax_download('US', 2024, expect: 200), tax_download('US', 2024, method: 'HEAD'), tax_download('US', 2024).merge('if_none_match' => 'last'),
                                        tax_download('us', 2024), tax_download('US', 2023, expect: 422), tax_download('US', 2021), tax_download('US', 2022),
                                        tax_download('US', 2025, expect: 404), tax_download('US', 2024, 'second-token'), tax_download('XX', 2024), tax_download('US', 'abc'),
                                        tax_download('US', 2024).merge('if_none_match' => 'last'), tax_download('US', 2024, 'personal-token'),
                                        tax_step(api('/api/v1/tax/reports/US/2024/download.json')), tax_download('US', 2024, headers: { 'Accept' => 'text/html' })],
                                       { '1_US_2024_crypto.csv' => TAX_REPORT, '1_US_2023_crypto.csv.refused.json' => TAX_EXCESS,
                                         '1_US_2022_crypto.csv.refused.json' => '{"reason":', '1_US_2021_crypto.csv.refused.json' => TAX_TOKENIZED,
                                         '1_US_2025_broker.csv' => TAX_REPORT, '2_US_2025_crypto.csv' => TAX_REPORT }),
        'api_tax_download_empty' => tax_case([tax_download('US', 2024)], { '1_US_2024_crypto.csv' => '' }),
        # D-TAX-NOT-PORTED-GATE: a jurisdiction whose engine this build has not ported (validation answers first, as Rails).
        'api_tax_d_not_ported' => tax_case([tax_status('DE', 2025), tax_create({ country: 'DE', year: 'abc' }), tax_create({ country: 'de', year: 2025, force: 'yes' }),
                                            tax_create({ country: 'de', year: 2025 }, expect: 202), tax_status('DE', 2025), tax_download('DE', 2025)]),
        # A wealth snapshot (NL) needs no market data: Rails accepts it on an install without a provider; the US report is refused first.
        'api_tax_d_not_ported_wealth' => tax_case([tax_create(us, expect: 422), tax_create({ country: 'NL', year: 2025 }, expect: 202), tax_status('NL', 2025)], {}, {},
                                                  'market_data' => 'none'),
        # The CoinGecko gate (PLAN-A12 r1 item 8, until A17): historical prices are fetched from Deltabadger market data only.
        'api_tax_d_coingecko' => tax_case([tax_create({ country: 'US', year: 'abc' }), tax_create(us, expect: 202), tax_status], {}, {}, 'market_data' => 'coingecko')
      )
      grid
    end

    def build(dir, template, scenario)
      jobs = super
      scenario.fetch('tax_fixtures', {}).each do |name, body|
        path = File.join(dir, 'tmp', 'tax_reports', name)
        FileUtils.mkdir_p(File.dirname(path))
        File.write(path, body)
      end
      jobs
    end

    # The owner's ledger on Alpaca (1) and Kraken (3), ids 1 to 13 in this order, and the second user's row 14:
    #  1 a USD-quoted buy (venue valued) with a fee and a formula-like description; 2 an airdrop with a stated price;
    #  3/4 a withdrawal and its one matching deposit; 5 a withdrawal with two deposits that match (6, 7);
    #  8/9 a linked pair; 10 a deposit nothing matches; 11/12 a buy and its cash leg in one group, at one instant;
    #  13 a reward with a tiny amount and fee.
    def api_ledger(owner, second, kind)
      return if kind == 'none'

      alpaca, kraken = Exchange.find_by!(name: 'Alpaca'), Exchange.find_by!(name: 'Kraken')
      at = ->(text) { Time.iso8601(text) }
      row = lambda do |user, exchange, entry_type, base, amount, time, **extra|
        AccountTransaction.create!({ user:, exchange:, entry_type:, base_currency: base, base_amount: amount, transacted_at: at.(time), raw_data: {} }.merge(extra))
      end
      if kind.start_with?('cap_')
        count = Integer(kind.delete_prefix('cap_'))
        now = Time.current
        rows = Array.new(count) do |i|
          { user_id: owner.id, exchange_id: alpaca.id, entry_type: 0, base_currency: 'BTC', base_amount: '0.001', quote_currency: 'USD', quote_amount: '60',
            transacted_at: at.('2026-03-01T00:00:00Z') + (i / 2).minutes, tx_id: "cap-#{i}", raw_data: {}, manual_values: {}, created_at: now, updated_at: now }
        end
        AccountTransaction.insert_all!(rows)
        return
      end
      row.(owner, alpaca, :buy, 'BTC', '0.5', '2026-09-01T10:00:00Z', quote_currency: 'USD', quote_amount: '30000', fee_currency: 'USD', fee_amount: '1.5',
                                                                     tx_id: 't-1', description: '=1+1')
      row.(owner, alpaca, :airdrop, 'ETH', '2', '2026-09-02T10:00:00Z', manual_values: { 'price' => '1800.5' })
      row.(owner, kraken, :withdrawal, 'BTC', '0.3', '2026-09-03T12:00:00Z', fee_currency: 'BTC', fee_amount: '0.0001')
      row.(owner, alpaca, :deposit, 'BTC', '0.2999', '2026-09-04T12:00:00Z')
      row.(owner, kraken, :withdrawal, 'ETH', '1', '2026-09-05T12:00:00Z')
      row.(owner, alpaca, :deposit, 'ETH', '1', '2026-09-06T12:00:00Z')
      row.(owner, alpaca, :deposit, 'ETH', '0.9', '2026-09-07T12:00:00Z')
      row.(owner, kraken, :withdrawal, 'SOL', '10', '2026-08-01T12:00:00Z')
      row.(owner, alpaca, :deposit, 'SOL', '10', '2026-08-02T12:00:00Z')
      AccountTransaction.find(8).update!(linked_transaction_id: 9)
      row.(owner, alpaca, :deposit, 'DOGE', '5', '2026-07-01T12:00:00Z')
      row.(owner, kraken, :buy, 'ADA', '100', '2026-09-08T09:00:00Z', group_id: 'g-1')
      row.(owner, kraken, :sell, 'USD', '50', '2026-09-08T09:00:00Z', group_id: 'g-1')
      row.(owner, alpaca, :staking_reward, 'ETH', '0.000000123456789', '2026-09-09T08:00:00Z', fee_currency: 'ETH', fee_amount: '0.00000001')
      row.(second, alpaca, :deposit, 'BTC', '1', '2026-09-03T13:00:00Z')
      return unless kind == 'many'

      201.times { |i| row.(owner, kraken, :buy, 'BTC', '1', (at.('2026-06-01T00:00:00Z') + i).iso8601, quote_currency: 'USD', quote_amount: '10000', tx_id: "many-#{i}") }
    end

    # Keys the sync reads (ApiKey.reading): 'two' adds a Kraken read-only key beside the install's Alpaca one; 'fallback' adds
    # an incorrect Kraken trading key and a correct read-only one (the read-only key is read); 'withdrawal' only a withdrawal key.
    def api_tracker_keys(owner, kind)
      kraken = Exchange.find_by!(name: 'Kraken')
      key = ->(exchange, key_type, status = 'correct') { ApiKey.new(user: owner, exchange:, key: 'k', secret: 's', status:, key_type:).save!(validate: false) }
      case kind
      when 'two' then key.(kraken, :read_only)
      when 'fallback' then key.(kraken, :trading, 'incorrect') && key.(kraken, :read_only)
      when 'withdrawal' then key.(Exchange.find_by!(name: 'Alpaca'), :withdrawal)
      end
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
