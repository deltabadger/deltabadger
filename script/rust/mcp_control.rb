# Synthetic M4 recorder; Rails application code remains the oracle, unchanged.
source = Rails.root.join('script/rust/mcp.rb').read.split("command, root = ARGV\ncase command\n").first
raise 'M2 recorder boundary changed' unless source.include?('module McpParity')

# The pinned, local recorder source supplies shared fixtures; no external code is evaluated.
eval(source, TOPLEVEL_BINDING, Rails.root.join('script/rust/mcp.rb').to_s) # rubocop:disable Security/Eval
module McpControl
  extend ActiveSupport::Testing::TimeHelpers

  module_function

  NAMES = %w[stop_bot archive_bot unarchive_bot delete_bot update_bot_settings start_bot].freeze
  # A10: the create tools, on and granted beside the control tools.
  CREATE = %w[create_bot create_index_bot create_signal_bot].freeze
  TABLES = (McpParity::TABLES + %w[bot_index_assets bot_activity_logs users account_transactions]).uniq.freeze
  JSON_COLUMNS = { 'bots' => %w[settings transient_data], 'bot_activity_logs' => %w[details] }.freeze
  def seed(dir)
    Pages.connect(dir)
    McpParity.configure
    travel_to(Time.iso8601(Pages::AT), with_usec: true) do
      owner = User.new(Pages.owner.merge(email: 'owner@example.com', password: Pages::PASSWORD))
      owner.save!(validate: false)
      second = User.new(Pages.owner.merge(email: 'second@example.com', password: Pages::PASSWORD))
      second.save!(validate: false)
      Pages.alpaca({})
      allowed = ENV.key?('M4_TOOLS') ? ENV.fetch('M4_TOOLS').split(',') : NAMES + CREATE
      app = Doorkeeper::Application.create!(name: 'M4', uid: 'm2-client', redirect_uri: 'http://localhost/cb', confidential: false, scopes: 'mcp api')
      { 'm2-token' => [owner, 'mcp'], 'api-token' => [owner, 'api'], 'second-token' => [second, 'mcp'] }.each do |token, (user, scopes)|
        Doorkeeper::AccessToken.create!(application: app, resource_owner_id: user.id, scopes:, expires_in: 3600).update_columns(token:)
        ConnectedClient.find_or_create_by!(user:, oauth_application: app).update!(mcp_tools: allowed)
      end
      owner.update_columns(mcp_settings: { tool_permissions: (NAMES + CREATE).to_h { |n| [n, true] }, dry_run: false })
      second.update_columns(mcp_settings: { tool_permissions: NAMES.to_h { |n| [n, true] } })
      Pages.bot('kind' => 'coins', 'columns' => { 'status' => 'stopped', 'label' => 'Control' })
      Pages.bot('kind' => 'index', 'columns' => { 'status' => 'stopped', 'label' => 'Index' })
      Bot.update_all(updated_at: Time.iso8601('2026-01-01T00:00:00Z'))
      # The ETH ticker of the basket's exchange: what a trading condition names in *_in_ticker_id.
      basket = Bot.find(1)
      @eth_ticker = Ticker.where(exchange_id: basket.exchange_id, base_asset_id: 17, quote_asset_id: basket.quote_asset_id).pick(:id)
    end
    File.write(File.join(dir, 'secret_key_base'), Rails.application.secret_key_base)
    ActiveRecord::Base.connection_pool.disconnect!
  end

  def set_status(status)
    "UPDATE bots SET status=#{status}, started_at='2026-09-09 12:00:00' WHERE id=1"
  end

  def tool(name, args = {}, sql = []) = McpParity.tool(name, { bot_id: 1 }.merge(args)).merge('sql' => sql)

  def scenarios
    s = {}
    NAMES.each do |name|
      (0..7).each do |status|
        s["#{name}_status_#{status}"] =
          McpParity.ready + [tool(name, name == 'update_bot_settings' ? { label: 'Renamed' } : {}, [set_status(status)])]
      end
      s["#{name}_missing"] = McpParity.ready + [tool(name, { bot_id: 999 })]
      s["#{name}_huge_id"] = McpParity.ready + [tool(name, { bot_id: 1e30 })]
      s["#{name}_fractional_id"] =
        McpParity.ready + [tool(name, { bot_id: 1.9 }, [set_status(if name == 'stop_bot'
                                                                     1
                                                                   else
                                                                     name == 'unarchive_bot' ? 7 : 2
                                                                   end)])]
      s["#{name}_owner"] = McpParity.ready + [tool(name).merge('headers' => tool(name)['headers'].merge('Authorization' => 'Bearer second-token'))]
      s["#{name}_grant"] = McpParity.ready + [tool(name, {}, ["UPDATE connected_clients SET mcp_tools='[]'"])]
      s["#{name}_disabled"] = McpParity.ready + [tool(name, {}, ["UPDATE users SET mcp_settings='{}'"])]
      s["#{name}_scope"] = McpParity.ready + [tool(name).merge('headers' => tool(name)['headers'].merge('Authorization' => 'Bearer api-token'))]
      [{}, { bot_id: nil }, { bot_id: '1' }, { bot_id: true }, { bot_id: [] }, { bot_id: 1, extra: true }].each_with_index do |args, i|
        s["#{name}_schema_#{i}"] = McpParity.ready + [McpParity.tool(name, args)]
      end
      status = case name
               when 'stop_bot'
                 1
               when 'unarchive_bot'
                 7
               else
                 name == 'start_bot' ? 0 : 2
               end
      s["legacy_#{name}"] =
        McpParity.ready + [tool(name, name == 'update_bot_settings' ? { label: 'Legacy rename' } : {},
                                [set_status(status), "UPDATE bots SET settings=json_remove(settings,'$.limit_order_pcnt_distance'),transient_data='{}' WHERE id=1"])] # rubocop:disable Layout/LineLength -- Literal transcript data keeps request values visible together.
    end
    s['lifecycle_repeat'] = McpParity.ready + [tool('start_bot', {}, [set_status(0)]), tool('start_bot'), tool('stop_bot'), tool('stop_bot'),
                                               tool('archive_bot'), tool('archive_bot'), tool('unarchive_bot'), tool('unarchive_bot'), tool('delete_bot'), tool('delete_bot')] # rubocop:disable Layout/LineLength -- Literal transcript data keeps request values visible together.
    allowed = ENV.key?('M4_TOOLS') ? ENV.fetch('M4_TOOLS').split(',') : NAMES + CREATE
    reads = %w[list_bots get_bot_details list_exchanges get_exchange_balances get_portfolio_summary list_transactions
               list_open_orders list_tax_jurisdictions]
    registered = AppConfig::MCP_TOOL_DEFAULTS.keys.select { |name| reads.include?(name) || allowed.include?(name) }
    grant = "UPDATE connected_clients SET mcp_tools=#{ActiveRecord::Base.connection.quote(registered.to_json)}"
    s['registry'] = McpParity.ready + [McpParity.session_rpc('tools/list').merge('sql' => [grant])]
    [0, 10, 20].each do |offset|
      s['registry'] << McpParity.session_rpc('tools/list', { cursor: Base64.urlsafe_encode64(offset.to_s, padding: false) })
    end
    s['registry'] += [McpParity.tool('create_bot'), McpParity.tool('create_index_bot'), McpParity.tool('create_signal_bot')]
    # A10: create_bot, create_index_bot, create_signal_bot (BotApi::Bots::Create*, as POST /api/v1/bots runs them).
    make = ->(name, args, sql = []) { McpParity.tool(name, args).merge('sql' => sql) }
    # ND100 offered on Alpaca with its members (Index::ExchangeAvailability), as pages_api.rb's index_alpaca.
    nasdaq = ["UPDATE indices SET available_exchanges=json_object('Exchanges::Alpaca',json(top_coins)) WHERE external_id='nasdaq-100'"]
    # Coins: the engine runs plain crypto on Alpaca; this fixture's stocks are untyped, which it refuses (D-API-CREATE-REFUSED).
    dca = { exchange_name: 'Alpaca', base_asset: 'BTC', quote_asset: 'usd', quote_amount: 25, interval: 'week' }
    index = { exchange_name: 'Alpaca', index: 'nasdaq-100', quote_asset: 'USD', quote_amount: 40, interval: 'day' }
    signal = { exchange_name: 'Alpaca', base_asset: 'AAPL', quote_asset: 'USD' }
    later = "#{(Time.iso8601(Pages::AT) + 3.days).to_date.iso8601}T09:30:00Z"
    s['create_dca'] = McpParity.ready + [make.('create_bot', dca), make.('create_bot', dca.merge(quote_amount: 0.00005, label: 'Tiny <b>'))]
    s['create_dca_basket'] = McpParity.ready + [make.('create_bot', dca.except(:base_asset).merge(assets: 'BTC:60,eth:40', interval: 'hour'))]
    s['create_dca_scheduled'] = McpParity.ready + [make.('create_bot', dca.merge(start_at: later)), make.('create_index_bot', index.merge(start_at: later), nasdaq)]
    s['create_dca_refusals'] = McpParity.ready + [
      make.('create_bot', {}), make.('create_bot', dca.merge(quote_amount: '25')), make.('create_bot', dca.merge(interval: 'year')),
      make.('create_bot', dca.merge(exchange_name: 'Nope')), make.('create_bot', dca.merge(base_asset: 'DOGE')),
      make.('create_bot', dca.except(:base_asset).merge(assets: 'BTC:60,ETH:30')), make.('create_bot', dca.merge(start_at: '2026-09-01T10:00:00Z'))
    ]
    s['create_index'] = McpParity.ready + [make.('create_index_bot', index, nasdaq), make.('create_index_bot', index.merge(num_coins: 5, allocation_flattening: 0.25))]
    s['create_index_refusals'] = McpParity.ready + [make.('create_index_bot', index), make.('create_index_bot', index.merge(index: 'nope'), nasdaq),
                                                    make.('create_index_bot', index.merge(num_coins: 2.5)), make.('create_index_bot', index.merge(allocation_flattening: 2))]
    s['create_signal'] = McpParity.ready + [make.('create_signal_bot', signal), make.('create_signal_bot', signal.merge(label: 'Signals'))]
    s['create_signal_refusals'] = McpParity.ready + [make.('create_signal_bot', {}), make.('create_signal_bot', signal.merge(base_asset: 'DOGE')),
                                                     make.('create_signal_bot', signal.merge(exchange_name: 'Interactive Brokers'))]
    # D-API-CREATE-REFUSED over MCP: a stock this fixture leaves untyped, which the engine's guard does not run.
    s['create_refused_untyped_stock'] = McpParity.ready + [make.('create_bot', dca.merge(base_asset: 'AAPL'))]
    # LIVE_AUTOMATION_TOOLS under paper trading, a tool switched off, a tool the client was not granted.
    paper = ["UPDATE users SET mcp_settings=json_set(mcp_settings,'$.dry_run',json('true')) WHERE email='owner@example.com'"]
    s['create_paper_trading'] = McpParity.ready + [make.('create_bot', dca, paper), make.('create_index_bot', index), make.('create_signal_bot', signal)]
    s['create_disabled'] = McpParity.ready + [make.('create_bot', dca, ["UPDATE users SET mcp_settings='{}' WHERE email='owner@example.com'"])]
    s['create_grant'] = McpParity.ready + [make.('create_signal_bot', signal, ["UPDATE connected_clients SET mcp_tools='[]'"])]
    { none: {}, blank: { label: '  ', allocations: ' ' }, label: { label: 'A <b>&"😀' }, same: { label: 'Control' },
      amount: { quote_amount: 37.25 }, amount_zero: { quote_amount: 0 }, amount_negative: { quote_amount: -1 },
      amount_tiny: { quote_amount: 0.00005 }, amount_large: { quote_amount: 1e15 },
      index_only: { num_coins: 3 }, flat_only: { allocation_flattening: 0.5 },
      weights: { allocations: 'BTC:70,ETH:30' }, reverse: { allocations: 'eth:40,btc:60' },
      weights_id: { allocations: '16:70,17:30' }, malformed: { allocations: 'BTC' },
      repeated: { allocations: 'BTC:50,btc:50' }, repeated_id: { allocations: 'BTC:50,16:20,ETH:30' },
      unknown: { allocations: 'DOGE:100' }, missing: { allocations: 'BTC:100' },
      unbalanced: { allocations: 'BTC:60,ETH:30' }, tolerance: { allocations: 'BTC:60,ETH:39.9' },
      excess: { allocations: 'BTC:101,ETH:0' }, exponent: { allocations: 'BTC:7e1,ETH:30' },
      negative: { allocations: 'BTC:-1,ETH:101' }, decimal: { allocations: 'BTC:12.34567890123456789,ETH:87.65432109876543211' },
      ordered: { quote_amount: 0, num_coins: 3, allocations: 'x' } }.each do |name, args|
      s["settings_#{name}"] = McpParity.ready + [tool('update_bot_settings', args)]
    end
    [0, 1, 2, 3, 12, 13, 100, 101, 2.5, 1.0000000000000002].each do |n|
      s["settings_index_count_#{n}"] = McpParity.ready + [tool('update_bot_settings', { bot_id: 2, num_coins: n })]
    end
    [-1, 0, 0.25, 1, 1.1].each do |n|
      s["settings_index_flat_#{n}"] = McpParity.ready + [tool('update_bot_settings', { bot_id: 2, allocation_flattening: n })]
    end
    s['settings_index_weights'] = McpParity.ready + [tool('update_bot_settings', { bot_id: 2, allocations: 'BTC:100' })]
    s['settings_typed_errors'] =
      McpParity.ready + [tool('update_bot_settings', { quote_amount: '12' }), tool('update_bot_settings', { label: 2 }),
                         tool('update_bot_settings', { allocations: { 'BTC' => 70, 'ETH' => 30 } })]
    s['start_fresh'] = McpParity.ready + [tool('start_bot', {}, [set_status(0)])]
    # start_fresh is the web restart dialog's choice: absent or true skips, false continues.
    s['start_fresh_stopped'] = McpParity.ready + [tool('start_bot', {}, [set_status(2)])]
    s['start_fresh_true_stopped'] = McpParity.ready + [tool('start_bot', { start_fresh: true }, [set_status(2)])]
    s['start_continue_created'] = McpParity.ready + [tool('start_bot', { start_fresh: false }, [set_status(0)])]
    [nil, 'false', 0].each_with_index do |flag, i|
      s["start_fresh_schema_#{i}"] = McpParity.ready + [tool('start_bot', { start_fresh: flag }, [set_status(2)])]
    end
    # Every start of a bot that can sell waits for the wash-sale answer (BotApi::Bots::Start.wash_sale_question_owed?).
    undecided = "UPDATE users SET wash_sale_enabled=NULL WHERE email='owner@example.com'"
    { buy_only: [],
      selling: ["UPDATE bots SET settings=json_set(settings,'$.direction','selling') WHERE id=1"],
      rebalance: ["UPDATE bots SET settings=json_set(settings,'$.rebalance_enabled',json('true'),'$.rebalance_threshold',0.05) WHERE id=1"],
      armed: ["UPDATE bots SET settings=json_set(settings,'$.indicator_limited',json('true'),'$.indicator_limit_action','start_selling') WHERE id=1"],
      held: ["INSERT OR IGNORE INTO transactions (id,bot_id,exchange_id,base_asset_id,quote_asset_id,status,external_status,side,transaction_type,quote_amount,quote_amount_exec,amount,amount_exec,price,created_at,updated_at) VALUES (1,1,1,16,1,0,2,0,'REGULAR',20,20,0.001,0.001,20000,'2026-09-10 12:00:00','2026-09-10 12:00:00')"] }.each do |name, sql| # rubocop:disable Layout/LineLength -- Literal transcript data.
      s["start_wash_question_#{name}"] = McpParity.ready + [tool('start_bot', {}, [set_status(2), undecided] + sql)]
    end
    s['start_continue_empty'] = McpParity.ready + [tool('start_bot', { start_fresh: false }, [set_status(2)])]
    s['start_continue_within'] = McpParity.ready + [tool('start_bot', { start_fresh: false }, [set_status(2), "UPDATE bots SET started_at='2026-09-10 12:00:00', transient_data=json_set(transient_data,'$.last_action_job_at','2026-09-10T12:00:01Z') WHERE id=1", # rubocop:disable Layout/LineLength -- Literal transcript data keeps request values visible together.
                                                                                               "INSERT INTO transactions (bot_id,exchange_id,base_asset_id,quote_asset_id,status,external_status,side,transaction_type,quote_amount,quote_amount_exec,amount,amount_exec,price,created_at,updated_at) VALUES (1,1,16,1,0,2,0,'REGULAR',20,20,0.001,0.001,20000,'2026-09-10 12:00:00','2026-09-10 12:00:00')"])] # rubocop:disable Layout/LineLength -- Literal transcript data keeps request values visible together.
    negative_zero = tool('update_bot_settings', { bot_id: 2, allocation_flattening: 0.0 })
    negative_zero['body'] = negative_zero['body'].sub('"allocation_flattening":0.0', '"allocation_flattening":-0.0')
    raise 'negative-zero wire lost its sign' unless negative_zero['body'].include?('"allocation_flattening":-0.0')

    s['settings_index_negative_zero'] = McpParity.ready + [negative_zero]
    { nbsp_percent: "BTC:\u00a070,ETH:30", tab_percent: "BTC:\t70\t,ETH:30",
      nbsp_identifier: "\u00a0BTC\u00a0:70,ETH:30", tab_identifier: "\tBTC\t:70,ETH:30",
      nul_percent: "BTC:\u000070\u0000,ETH:30", nul_identifier: "\u0000BTC\u0000:70,ETH:30" }.each do |name, allocations|
      s["settings_r1_#{name}"] = McpParity.ready + [tool('update_bot_settings', { allocations: })]
    end
    %w[stop_bot archive_bot unarchive_bot delete_bot].each do |name|
      status = name == 'unarchive_bot' ? 7 : 1
      s["#{name}_r1_invalid"] = McpParity.ready + [tool(name, {}, [set_status(status),
                                                                   "UPDATE bots SET settings=json_set(settings,'$.rebalance_enabled',json('false'),'$.rebalance_threshold',0) WHERE id=1"])] # rubocop:disable Layout/LineLength -- Literal transcript data.
    end
    { missing: 'DELETE FROM api_keys', pending: 'UPDATE api_keys SET status=0', incorrect: 'UPDATE api_keys SET status=2' }.each do |name, sql|
      s["start_bot_r1_key_#{name}"] = McpParity.ready + [tool('start_bot', {}, [set_status(0), sql])]
    end
    negative_carry = "UPDATE bots SET transient_data=json_set(transient_data,'$.missed_quote_amount',-1) WHERE id=1"
    { amount: { quote_amount: 37.25 }, same_amount: { quote_amount: 20 }, label: { label: 'Carry label' } }.each do |name, args|
      s["settings_r2_negative_carry_#{name}"] = McpParity.ready + [tool('update_bot_settings', args, [negative_carry])]
    end
    s['start_bot_r2_negative_carry'] = McpParity.ready + [tool('start_bot', {}, [set_status(0), negative_carry])]
    s['settings_r2_unicode_symbol'] = McpParity.ready + [tool('update_bot_settings', { allocations: 'ſOL:70,ETH:30' },
                                                              ["UPDATE assets SET symbol='SOL' WHERE id=16"])]
    schedule = [set_status(2), "UPDATE bots SET settings=json_set(settings,'$.interval','hour','$.smart_intervaled',json('true'),'$.smart_interval_quote_amount',10), transient_data=json_set(transient_data,'$.last_action_job_at','2026-09-09T12:00:01Z') WHERE id=1"]
    s['settings_schedule_bounds'] = McpParity.ready + [tool('update_bot_settings', { quote_amount: 1e-18 }, schedule)]
    s['start_schedule_bounds'] = McpParity.ready + [tool('start_bot', {}, schedule + ["UPDATE bots SET settings=json_set(settings,'$.quote_amount',1e-18) WHERE id=1"])]
    s['settings_schedule_rails_invalid'] = McpParity.ready + [tool('update_bot_settings', { quote_amount: 0 }, schedule)]
    s['start_schedule_rails_invalid'] = McpParity.ready + [tool('start_bot', {}, schedule + ["UPDATE bots SET status=0,started_at=NULL,settings=json_set(settings,'$.quote_amount',0) WHERE id=1"])]
    # Paper trading on: start_bot refuses before any lookup, validation or write; other tools are unaffected.
    paper = "UPDATE users SET mcp_settings=json_set(mcp_settings,'$.dry_run',json('true')) WHERE email='owner@example.com'"
    [0, 1, 2].each { |status| s["paper_refused_status_#{status}"] = McpParity.ready + [tool('start_bot', {}, [set_status(status), paper])] }
    s['paper_refused_missing'] = McpParity.ready + [tool('start_bot', { bot_id: 999 }, [paper])]
    s['paper_refused_schema'] = McpParity.ready + [tool('start_bot', { bot_id: '1' }, [paper])]
    loose = "UPDATE users SET mcp_settings=json_set(mcp_settings,'$.dry_run','true')"
    s['paper_loose_string'] = McpParity.ready + [tool('start_bot', {}, [set_status(0), loose])]
    s['paper_stop_bot'] = McpParity.ready + [tool('stop_bot', {}, [set_status(1), paper])]
    # PLAN-wash-sale WS-3: Stop of a working basket, and of a working signal bot (bot 2 rewritten as one), on an account with
    # the rule on or the question unanswered (BotApi::Bots::Stop never asks the setting).
    signal = ["UPDATE bots SET type='Bots::Signal', status=1, started_at='2026-09-09 12:00:00', transient_data='{}', " \
              "settings=json_object('base_asset_id', 17, 'quote_asset_id', json_extract((SELECT settings FROM bots WHERE id=1), '$.quote_asset_id')) WHERE id=2",
              'DELETE FROM bot_index_assets WHERE bot_id=2',
              "INSERT INTO bot_signals (bot_id, amount, amount_type, direction, enabled, token, created_at, updated_at) VALUES " \
              "(2, 10, 0, 0, 1, 'mcp-wash-buy', '2026-09-01 00:00:00', '2026-09-01 00:00:00'), (2, 50, 1, 1, 1, 'mcp-wash-sell', '2026-09-01 00:00:00', '2026-09-01 00:00:00')"]
    { 'rule_on' => 1, 'question' => 'NULL' }.each do |name, value|
      wash = "UPDATE users SET wash_sale_enabled=#{value} WHERE email='owner@example.com'"
      s["stop_bot_wash_#{name}"] = McpParity.ready + [tool('stop_bot', {}, [set_status(1), wash])]
      s["stop_bot_signal_wash_#{name}"] = McpParity.ready + [tool('stop_bot', { bot_id: 2 }, signal + [wash])]
    end
    settings_form(s)
    order_scenarios(s)
    tracker_scenarios(s)
    tax_scenarios(s)
    prefixes = ENV.fetch('M4', '').split(',')
    selected = s.select { |name, _| prefixes.empty? || prefixes.any? { |p| name.start_with?(p) } }
    raise 'empty M4 scenario filter' if selected.empty?

    selected
  end

  # PLAN-A9 slice O1: the order tools up to the venue. Granted and switched on before the session starts. Bot 1 (the coins
  # basket) gets an open order and an imported one; bots 3 to 5 are Alpaca signal bots on QQQM: working, stopped, and working
  # with an unresolved placement. 'paper' turns paper trading on: every answer is prefixed, and the orders that pass every
  # check answer as dry runs (no venue is read but Rails' price read for a standalone market order, refused below).
  def order_ready(extra = [])
    quote = ->(v) { ActiveRecord::Base.connection.quote(v) }
    owner = "(SELECT id FROM users WHERE email='owner@example.com')"
    on = McpParity::ORDER_NAMES.map { |n| "'$.tool_permissions.#{n}',json('true')" }.join(',')
    signal = lambda do |id, status, transient = '{}'|
      "INSERT INTO bots (id,type,user_id,exchange_id,status,label,settings,transient_data,started_at,created_at,updated_at) VALUES " \
        "(#{id},'Bots::Signal',#{owner},(SELECT id FROM exchanges WHERE name='Alpaca'),#{status},'Signal #{id}'," \
        "json_object('base_asset_id',(SELECT id FROM assets WHERE symbol='QQQM'),'quote_asset_id',(SELECT id FROM assets WHERE symbol='USD'))," \
        "#{quote.(transient)},'2026-09-09 12:00:00','2026-01-01 00:00:00','2026-01-01 00:00:00')"
    end
    order = lambda do |id, external, side|
      "INSERT INTO transactions (id,bot_id,exchange_id,base_asset_id,quote_asset_id,status,external_status,side,order_type,transaction_type,amount,price," \
        "external_id,base,quote,created_at,updated_at) VALUES (#{id},1,(SELECT id FROM exchanges WHERE name='Alpaca'),(SELECT id FROM assets WHERE symbol='QQQM')," \
        "(SELECT id FROM assets WHERE symbol='USD'),0,1,#{side},1,'REGULAR',0.12247,408.25,'#{external}','QQQM','USD','2026-09-10 11:00:00','2026-09-10 11:00:00')"
    end
    marker = { 'signal_placement_unresolved' => { 'id' => 'marker-1', 'at' => '2026-09-10T11:00:00Z', 'side' => 'buy', 'error' => 'timeout' } }.to_json
    fixtures = ["UPDATE connected_clients SET mcp_tools=#{quote.((NAMES + McpParity::ORDER_NAMES).to_json)}",
                "UPDATE users SET mcp_settings=json_set(mcp_settings,#{on})",
                signal.(3, 1), signal.(4, 2), signal.(5, 1, marker), order.(1, 'venue-open-1', 0), order.(2, 'imported_venue-2', 1)]
    [McpParity.init.merge('sql' => fixtures + extra), McpParity.session_rpc('notifications/initialized', {}, nil)]
  end

  def order_scenarios(s)
    paper = "UPDATE users SET mcp_settings=json_set(mcp_settings,'$.dry_run',json('true')) WHERE email='owner@example.com'"
    call = ->(name, args) { McpParity.tool(name, args) }
    pair = { exchange_name: 'Alpaca', base_asset: 'QQQM', quote_asset: 'USD' }
    refusals = [call.('market_buy', { amount: 10 }), call.('market_buy', { exchange_name: 'Nope<x>', base_asset: 'QQQM', quote_asset: 'USD', amount: 10 }),
                call.('market_sell', { exchange_name: 'interactive brokers', base_asset: 'QQQM', quote_asset: 'USD', amount: 1 }),
                call.('limit_buy', { exchange_name: 'alpaca', base_asset: 'qqqm', quote_asset: 'eur', amount: 10, price: 400 }),
                call.('limit_sell', { exchange_name: 'Alpaca', base_asset: ' ', quote_asset: 'USD', amount: 1, price: 400 }),
                call.('market_buy', { bot_id: 999, amount: 10 }), call.('market_buy', { bot_id: 1, amount: 10 }), call.('market_buy', { bot_id: 4, amount: 10 }),
                call.('market_sell', { bot_id: 5, amount: 1 }), call.('market_buy', { bot_id: 3.5, amount: 10 }), call.('market_buy', { bot_id: 0, amount: 10 }),
                call.('market_buy', { bot_id: 3, amount: 0 }), call.('market_buy', { bot_id: 3, amount: 10, amount_type: 'nope' }),
                call.('market_buy', { bot_id: 3, amount: 10, base_asset: 'ibit', exchange_name: 'kraken' }),
                call.('limit_sell', pair.merge(bot_id: 3, amount: 1, price: 400)),
                call.('cancel_order', { order_id: '' }), call.('cancel_order', { order_id: 'abc' }), call.('cancel_order', { order_id: 'abc', exchange_name: 'Nope' }),
                call.('cancel_order', { order_id: 'abc', exchange_name: 'Interactive Brokers' }), call.('cancel_order', { order_id: '2' }),
                call.('cancel_order', { order_id: '999' }), call.('market_buy', {}), call.('cancel_order', { order_id: 7 }),
                call.('market_buy', { amount: 10, extra: true })]
    s['order_refusals'] = order_ready + refusals
    s['order_paper_refusals'] = order_ready([paper]) + refusals
    s['order_paper_placed'] = order_ready([paper]) + [
      call.('market_buy', { bot_id: 3, amount: 10 }), call.('market_sell', { bot_id: 3, amount: 0.5, amount_type: 'base' }),
      call.('market_sell', { bot_id: 3.0, amount: 2, exchange_name: 'ALPACA', base_asset: 'qqqm' }),
      call.('market_buy', { bot_id: 3, amount: 0.00005, amount_type: 'base' }),
      call.('limit_buy', pair.merge(amount: 10, price: 400)), call.('limit_sell', pair.merge(amount: 0.5, price: 401.5, amount_type: 'base')),
      call.('market_buy', { exchange_name: 'alpaca', base_asset: 'qqqm', quote_asset: 'usd', amount: 10 }),
      call.('market_sell', pair.merge(amount: 1))
    ]
    # The lock's last day in the caller's zone: 2026-09-24 00:00 UTC is still the 23rd in California.
    wash = ["UPDATE users SET wash_sale_enabled=1, wash_sale_jurisdiction='US', time_zone='Pacific Time (US & Canada)' WHERE email='owner@example.com'",
            "INSERT INTO wash_sale_locks (user_id,asset_id,buy_locked_until,source,created_at,updated_at) VALUES ((SELECT id FROM users WHERE email='owner@example.com')," \
            "(SELECT id FROM assets WHERE symbol='QQQM'),'2026-09-25 00:00:00','order','2026-09-01 00:00:00','2026-09-01 00:00:00')"]
    s['order_wash_locked'] = order_ready(wash) + [call.('market_buy', pair.merge(amount: 10)), call.('market_buy', { bot_id: 3, amount: 10 })]
    s['order_key_pending'] = order_ready(['UPDATE api_keys SET status=3']) + [call.('market_buy', { bot_id: 3, amount: 10 }), call.('market_buy', pair.merge(amount: 10))]
    # D-ORDER-DRY-CANCEL: Rails' Exchange::Dryable raises on a dry-run cancel; this build answers the success text.
    s['order_dry_cancel'] = order_ready([paper]) + [call.('cancel_order', { order_id: '1' })]
    s['order_dry_cancel_external'] = order_ready([paper]) + [call.('cancel_order', { order_id: 'venue-9', exchange_name: 'alpaca' })]
  end

  # update_bot_settings speaks the settings page's fields (BotApi::Bots::SettingsFields): every field on the
  # basket (bot 1) and the index (bot 2), each switch off again, the four fields null clears, starting times,
  # the selling side, partial and unbalanced weights, members, and the schema's refusals.
  def form_value(field)
    case field.name
    when /_in_ticker_id\z/ then @eth_ticker
    when 'add_asset_id' then 1 # the quote asset: the page's own composition check refuses it
    when 'remove_asset_id' then 17
    when 'exchange_id' then 1
    when 'num_coins' then 5
    when 'label' then 'Form'
    when 'allocations' then 'BTC:70'
    when 'start_time_of_day' then '09:30'
    when 'start_at' then '2026-12-01T10:00'
    when 'limit_order_pcnt_distance', 'price_drop_limit', 'sell_price_drop_limit', 'rebalance_threshold' then 5
    when 'allocation_flattening' then 0.25
    when /_in_period\z/ then 21
    else
      if field.enum then field.enum.last
      elsif field.type == 'boolean' then true
      else 12.5
      end
    end
  end

  def settings_form(s)
    fields = BotApi::Bots::SettingsFields::FIELDS
    selling = "UPDATE bots SET settings=json_set(settings,'$.direction','selling') WHERE id=1"
    fields.each do |field|
      [1, 2].each do |bot_id|
        s["settings_form_#{field.name}_#{bot_id}"] = McpParity.ready + [tool('update_bot_settings', { bot_id:, field.name => form_value(field) })]
      end
      if field.type == 'boolean'
        on = "UPDATE bots SET settings=json_set(settings,'$.#{field.name}',json('true')) WHERE id IN (1,2)"
        s["settings_form_off_#{field.name}"] = McpParity.ready + [tool('update_bot_settings', { field.name => false }, [on])]
      end
      next unless field.nullable

      set = "UPDATE bots SET settings=json_set(settings,'$.direction','selling','$.sell_amount',0.5,'$.sell_quote_amount',25," \
            "'$.rebalance_threshold',0.05,'$.start_at','2026-12-01T10:00:00Z') WHERE id=1"
      s["settings_form_null_#{field.name}"] = McpParity.ready + [tool('update_bot_settings', { field.name => nil }, [set])]
      s["settings_form_omitted_#{field.name}"] = McpParity.ready + [tool('update_bot_settings', { label: 'Other' }, [set])]
      s["settings_form_set_#{field.name}"] = McpParity.ready + [tool('update_bot_settings', { field.name => form_value(field) }, [set])]
    end
    { weekday: { start_time_enabled: true, start_time_mode: 'monday', start_time_of_day: '09:30' },
      hour: { start_time_enabled: true, start_time_mode: 'hour', start_time_of_day: '23:59' },
      date_zoneless: { start_time_enabled: true, start_time_mode: 'date', start_at: '2026-12-01T10:00' },
      date_offset: { start_time_enabled: true, start_time_mode: 'date', start_at: '2026-12-01T10:00:00+02:00' },
      date_past: { start_time_enabled: true, start_time_mode: 'date', start_at: '2020-01-01T00:00' },
      rules: { limit_ordered: true, limit_order_pcnt_distance: 0.5, smart_intervaled: true, smart_interval_quote_amount: 5,
               rebalance_enabled: true, rebalance_threshold: 7.5, quote_amount_limited: true, quote_amount_limit: 300 },
      price_between: { price_limited: true, price_limit_mode: 'restrict', price_limit_value_condition: 'between',
                       price_limit_range_lower_bound: 1000, price_limit_range_upper_bound: 2000 },
      partial: { allocations: 'BTC:70' }, thirds: { allocations: 'BTC:33.3' }, full: { allocations: 'BTC:33.3,ETH:66.7' },
      add_member: { add_asset_id: 1 }, remove_member: { remove_asset_id: 17 }, normalize: { normalize_allocations: true },
      weighting: { weighting: 'market_cap' }, interval: { interval: 'week' },
      schema_enum: { interval: 'fortnight' }, schema_switch: { limit_ordered: 'yes' }, schema_text: { start_time_mode: 3 },
      schema_number: { price_limit: '50000' }, schema_null: { quote_amount: nil } }.each do |name, args|
      s["settings_form_case_#{name}"] = McpParity.ready + [tool('update_bot_settings', args)]
    end
    s['settings_form_case_market_cap_weights'] =
      McpParity.ready + [tool('update_bot_settings', { allocations: 'BTC:60,ETH:40' },
                              ["UPDATE bots SET settings=json_set(settings,'$.weighting','market_cap') WHERE id=1"])]
    # Plan CB T5: with every member capped, the save derives the members' weights from the caps (apply_market_cap_weights);
    # switched back to manual, the sliders are the weights again.
    caps = "UPDATE assets SET market_cap=CASE symbol WHEN 'BTC' THEN 1300000000000 WHEN 'ETH' THEN 400000000000 END WHERE symbol IN ('BTC','ETH')"
    s['settings_form_case_market_cap_capped'] = McpParity.ready + [tool('update_bot_settings', { weighting: 'market_cap' }, [caps])]
    s['settings_form_case_market_cap_capped_allocations'] =
      McpParity.ready + [tool('update_bot_settings', { allocations: 'BTC:60,ETH:40' },
                              [caps, "UPDATE bots SET settings=json_set(settings,'$.weighting','market_cap') WHERE id=1"])]
    s['settings_form_case_market_cap_off'] =
      McpParity.ready + [tool('update_bot_settings', { weighting: 'manual' },
                              [caps, "UPDATE bots SET settings=json_set(settings,'$.weighting','market_cap') WHERE id=1",
                               "UPDATE bot_index_assets SET target_allocation=0.764706 WHERE bot_id=1 AND asset_id=(SELECT id FROM assets WHERE symbol='BTC')"])]
    s['settings_form_case_unbalanced_normalize'] =
      McpParity.ready + [tool('update_bot_settings', { normalize_allocations: true },
                              ["UPDATE bots SET settings=json_set(settings,'$.allocations',json('{\"16\":0.6,\"17\":0.6}')) WHERE id=1"])]
    { sell_amount: { sell_amount: 0.25, sell_interval: 'week' }, sell_quote: { sell_quote_amount: 40 },
      sell_trigger: { sell_price_limited: true, sell_price_limit_mode: 'flip', sell_price_limit: 900, sell_price_limit_in_ticker_id: @eth_ticker },
      sell_drop: { sell_price_drop_limited: true, sell_price_drop_limit: 5, sell_price_drop_limit_time_window_condition: 'seven_days' },
      sell_cap: { base_amount_limited: true, base_amount_limit: 2 } }.each do |name, args|
      s["settings_form_selling_#{name}"] = McpParity.ready + [tool('update_bot_settings', args, [selling])]
    end
    # Transport order is not catalogue order: the answer lists settings in catalogue order, never bot_id.
    s['settings_form_order'] = McpParity.ready + [McpParity.tool('update_bot_settings', { rebalance_enabled: true, label: 'Order', quote_amount: 30, bot_id: 1 })]
    s['settings_form_order_reversed'] =
      McpParity.ready + [McpParity.tool('update_bot_settings', fields.reverse.select { |f| %w[boolean].include?(f.type) && f.kinds.include?(:index) }
                                                                .to_h { |f| [f.name, true] }.merge(bot_id: 2))]
  end

  # PLAN-A11: the tracker tools and list_indices. Granted, switched on and the owner's zone set to Warsaw before the session
  # starts; the ledger inserted by SQL (both runtimes run the same statements). Rows on Alpaca (1) and Interactive Brokers (2):
  #  1 a USD-quoted buy (venue valued); 2 an airdrop with a stated price; 3/4 a withdrawal and its one deposit; 5 a withdrawal
  #  with two (6, 7); 8/9 a linked pair; 10 a deposit nothing matches; 11 the second user's; 12 a deposit at 01:00 Warsaw on
  #  2026-09-03 (23:00 UTC the day before). Then the DST windows (PLAN-A11 r3): per vector a pair 1 s inside the resolved edge
  #  (base <V>I) and a pair 1 s outside it (<V>O); the source side is the withdrawal (forward) or the deposit (backward).
  TRACKER_DST = [
    # [name, source side, source UTC, edge UTC]: V1 crosses the autumn change (337 h); V2/V3 land in the repeated hour from a
    # summer and a winter source; V4/V5 land in the spring gap.
    ['V1', :withdrawal, '2026-10-20 10:00:00', '2026-11-03 11:00:00'], ['V2', :withdrawal, '2026-10-11 00:30:00', '2026-10-25 00:30:00'],
    ['V3', :deposit, '2026-11-08 01:30:00', '2026-10-25 01:30:00'], ['V4', :withdrawal, '2026-03-15 01:30:00', '2026-03-29 01:30:00'],
    ['V5', :deposit, '2026-04-12 00:30:00', '2026-03-29 01:30:00']
  ].freeze

  def tracker_ready(extra = [])
    quote = ->(v) { ActiveRecord::Base.connection.quote(v) }
    owner = "(SELECT id FROM users WHERE email='owner@example.com')"
    second = "(SELECT id FROM users WHERE email='second@example.com')"
    on = McpParity::TRACKER_NAMES.map { |n| "'$.tool_permissions.#{n}',json('true')" }.join(',')
    row = lambda do |user, exchange, kind, base, amount, at, extra = {}|
      columns = { user_id: user, exchange_id: exchange, entry_type: AccountTransaction.entry_types.fetch(kind.to_s), base_currency: quote.(base),
                  base_amount: amount, transacted_at: quote.(at), raw_data: "'{}'", manual_values: "'{}'", created_at: "'2026-01-01 00:00:00'",
                  updated_at: "'2026-01-01 00:00:00'" }.merge(extra)
      "INSERT INTO account_transactions (#{columns.keys.join(',')}) VALUES (#{columns.values.join(',')})"
    end
    rows = [row.(owner, 1, :buy, 'BTC', '0.5', '2026-09-01 10:00:00', quote_currency: "'USD'", quote_amount: '30000', fee_currency: "'USD'", fee_amount: '1.5'),
            row.(owner, 1, :airdrop, 'ETH', '2', '2026-09-02 10:00:00', manual_values: quote.('{"price":"1800.5"}')),
            row.(owner, 2, :withdrawal, 'BTC', '0.3', '2026-09-03 12:00:00'), row.(owner, 1, :deposit, 'BTC', '0.2999', '2026-09-04 12:00:00'),
            row.(owner, 2, :withdrawal, 'ETH', '1', '2026-09-05 12:00:00'), row.(owner, 1, :deposit, 'ETH', '1', '2026-09-06 12:00:00'),
            row.(owner, 1, :deposit, 'ETH', '0.9', '2026-09-07 12:00:00'), row.(owner, 2, :withdrawal, 'SOL', '10', '2026-08-01 12:00:00', linked_transaction_id: 9),
            row.(owner, 1, :deposit, 'SOL', '10', '2026-08-02 12:00:00'), row.(owner, 1, :deposit, 'DOGE', '5', '2026-07-01 12:00:00'),
            row.(second, 1, :deposit, 'BTC', '1', '2026-09-03 13:00:00'), row.(owner, 1, :deposit, 'XRP', '1', '2026-09-02 23:00:00')]
    TRACKER_DST.each do |name, side, source, edge|
      edge = Time.iso8601("#{edge.tr(' ', 'T')}Z")
      { 'I' => (side == :withdrawal ? -1 : 1), 'O' => (side == :withdrawal ? 1 : -1) }.each do |suffix, offset|
        other = (edge + offset).utc.strftime('%Y-%m-%d %H:%M:%S')
        withdrawal, deposit = side == :withdrawal ? [source, other] : [other, source]
        rows << row.(owner, 2, :withdrawal, "#{name}#{suffix}", '1', withdrawal) << row.(owner, 1, :deposit, "#{name}#{suffix}", '1', deposit)
      end
    end
    fixtures = ["UPDATE connected_clients SET mcp_tools=#{quote.((NAMES + McpParity::TRACKER_NAMES).to_json)}",
                "UPDATE users SET mcp_settings=json_set(mcp_settings,#{on}), time_zone='Warsaw'"] + rows
    [McpParity.init.merge('sql' => fixtures + extra), McpParity.session_rpc('notifications/initialized', {}, nil)]
  end

  # The ids of TRACKER_DST's rows (13 on): per vector, the I pair then the O pair, withdrawal first.
  def tracker_dst_ids(name, suffix)
    first = 13 + (TRACKER_DST.index { |v| v[0] == name } * 4) + (suffix == 'I' ? 0 : 2)
    [first, first + 1]
  end

  def tracker_scenarios(s)
    call = ->(name, args = {}) { McpParity.tool(name, args) }
    list = ->(args = {}) { call.('list_account_transactions', args) }
    export = ->(args = {}) { call.('export_transactions_csv', args) }
    link = ->(id, linked = true) { call.('set_transfer_link', { transaction_id: id, linked: }) }
    price = ->(id, value) { call.('set_transaction_price', { transaction_id: id, price_usd: value }) }
    s['tracker_list'] = tracker_ready + [list.(), list.(exchange_id: 2), list.(exchange_id: 999), list.(exchange_id: 1.5), list.(from_date: '2026-09-03'),
                                         list.(to_date: '2026-09-02'), list.(from_date: '2026-09-02', to_date: '2026-09-04', exchange_id: 1),
                                         list.(from_date: 'invalid'), list.(to_date: '2024-02-30'), list.(from_date: ''), list.(entry_type: 'deposit'),
                                         list.(entry_type: 'withdrawal', limit: 2), list.(entry_type: ''), list.(limit: 1), list.(limit: 0), list.(limit: -3),
                                         list.(limit: 500), list.(limit: 1.9), list.(from_date: 2026), list.(extra: true)]
    # D-API-DATE-STRICT and D-API-ENTRY-TYPE (tests/mcp_control.rs tracker_difference).
    s['tracker_d_dates'] = tracker_ready + [list.(from_date: 'invalid'), list.(from_date: 'Sep 5'), list.(to_date: '20260903'), export.(from_date: '2026-09-02T10:00')]
    s['tracker_d_entry_type'] = tracker_ready + [list.(entry_type: 'bogus'), list.(entry_type: '4')]
    s['tracker_export'] = tracker_ready + [export.(), export.(exchange_id: 2), export.(from_date: '2026-09-03', to_date: '2026-09-04'), export.(exchange_id: 999),
                                           export.(from_date: 'invalid'), export.(from_date: '2030-01-01')]
    s['tracker_link'] = tracker_ready + [link.(3), link.(3), link.(4, false), link.(4, false), link.(4), link.(5), link.(10), link.(999), link.(11),
                                         link.(9), link.(8, false), link.(3.9, false), call.('set_transfer_link', { transaction_id: 3 }),
                                         call.('set_transfer_link', { transaction_id: 3, linked: 'true' })]
    # D-LINK-KIND.
    s['tracker_d_link_kind'] = tracker_ready + [link.(1), link.(2), link.(2, false)]
    s['tracker_dst_source_side'] = tracker_ready + TRACKER_DST.flat_map do |name, side, *|
      %w[I O].map { |suffix| link.(tracker_dst_ids(name, suffix)[side == :withdrawal ? 0 : 1]) }
    end
    s['tracker_dst_other_side'] = tracker_ready + TRACKER_DST.flat_map do |name, side, *|
      %w[I O].map { |suffix| link.(tracker_dst_ids(name, suffix)[side == :withdrawal ? 1 : 0]) }
    end
    s['tracker_price'] = tracker_ready + [price.(10, '12.5'), price.(10, '12.5'), price.(10, ''), price.(10, ''), price.(10, '-5'), price.(10, '1e3'),
                                          price.(10, '0'), price.(10, ' 7.50 '), price.(10, '0.1234567890123456789'), price.(1, '12'), price.(1, ''),
                                          price.(2, '  '), price.(999, '1'), price.(11, '1'), price.(10, 5), call.('set_transaction_price', { transaction_id: 10 })]
    s['tracker_sync'] = tracker_ready + [call.('sync_tracker'), call.('sync_tracker', { extra: 1 })]
    s['tracker_sync_none'] = tracker_ready(['DELETE FROM api_keys']) + [call.('sync_tracker')]
    # D-SYNC-VENUE: a reading key on Interactive Brokers.
    s['tracker_d_sync_venue'] = tracker_ready(["INSERT INTO api_keys (user_id,exchange_id,key_type,status,created_at,updated_at) VALUES " \
                                               "((SELECT id FROM users WHERE email='owner@example.com'),2,2,1,'2026-01-01 00:00:00','2026-01-01 00:00:00')"]) +
                                [call.('sync_tracker')]
    s['tracker_not_granted'] = tracker_ready(["UPDATE connected_clients SET mcp_tools=#{ActiveRecord::Base.connection.quote(NAMES.to_json)}"]) +
                               McpParity::TRACKER_NAMES.map { |n| call.(n) }
    s['tracker_disabled'] = tracker_ready(["UPDATE users SET mcp_settings=json_set(mcp_settings,#{McpParity::TRACKER_NAMES.map { |n| "'$.tool_permissions.#{n}',json('false')" }.join(',')})"]) +
                            McpParity::TRACKER_NAMES.map { |n| call.(n) }
    s['tracker_paper'] = tracker_ready(["UPDATE users SET mcp_settings=json_set(mcp_settings,'$.dry_run',json('true')) WHERE email='owner@example.com'"]) +
                         [price.(10, '3'), link.(3), call.('sync_tracker'), list.(limit: 2)]
    s['tracker_indices'] = tracker_ready + [call.('list_indices'), call.('list_indices', { exchange_name: 'alpaca' }), call.('list_indices', { exchange_name: 'Nope<x>' }),
                                            call.('list_indices', { exchange_name: ' ' })]
    s['tracker_indices_unconfigured'] = tracker_ready(["DELETE FROM app_configs WHERE key IN ('market_data_provider','market_data_url','market_data_token')"]) +
                                        [call.('list_indices')]
    s['tracker_second_user'] = tracker_ready + [list.().merge('headers' => list.()['headers'].merge('Authorization' => 'Bearer second-token'))]
  end

  # PLAN-A12 A12-1: the three tax report tools, granted and switched on for the owner. The first step may carry the
  # scenario's install state, applied by `grid` (tax_install): report and refusal files by name under <scenario>/tmp/tax_reports
  # ('tax_fixtures'), a report already generating ('tax_generating': an unfinished Solid Queue job under the job's concurrency
  # key; tests/mcp_control.rs holds the same as this build's request row) and the market data provider ('tax_market_data').
  # ActiveJob's test adapter holds no slot for an accepted create: no scenario asks again after one (see pages_api.rb).
  TAX_NAMES = McpParity::TAX_NAMES

  def tax_ready(extra = [], install = {})
    on = TAX_NAMES.map { |n| "'$.tool_permissions.#{n}',json('true')" }.join(',')
    fixtures = ["UPDATE connected_clients SET mcp_tools=#{ActiveRecord::Base.connection.quote((NAMES + TAX_NAMES).to_json)}",
                "UPDATE users SET mcp_settings=json_set(mcp_settings,#{on})"]
    [McpParity.init.merge('sql' => fixtures + extra).merge(install), McpParity.session_rpc('notifications/initialized', {}, nil)]
  end

  def tax_install(dir, install)
    install.fetch('tax_fixtures', {}).each do |name, body|
      path = File.join(dir, 'tmp', 'tax_reports', name)
      FileUtils.mkdir_p(File.dirname(path))
      File.write(path, body)
    end
    Pages.connect(dir)
    case install['tax_market_data']
    when 'none' then AppConfig.where(key: %w[market_data_provider market_data_url market_data_token]).delete_all
    when 'coingecko'
      AppConfig.where(key: %w[market_data_url market_data_token]).delete_all
      AppConfig.set('market_data_provider', 'coingecko')
    end
    if install['tax_generating']
      SolidQueue::Record.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production_queue.sqlite3'))
      SolidQueue::Job.create!(queue_name: 'low_priority', class_name: 'Tax::GenerateReportJob', arguments: {},
                              concurrency_key: Tax::GenerateReportJob.new(User.find_by!(email: 'owner@example.com').id, 'XX', 0).concurrency_key)
      SolidQueue::Record.connection_pool.disconnect!
    end
    ActiveRecord::Base.connection_pool.disconnect!
  end

  def tax_scenarios(s)
    report = Pages::ApiGrid::TAX_REPORT
    files = { '1_US_2024_crypto.csv' => report, '1_US_2023_crypto.csv.refused.json' => Pages::ApiGrid::TAX_EXCESS,
              '1_US_2022_crypto.csv.refused.json' => '{"reason":', '1_US_2021_crypto.csv.refused.json' => Pages::ApiGrid::TAX_TOKENIZED,
              '1_US_2025_broker.csv' => report, '2_US_2025_crypto.csv' => report }
    ready = { '1_US_2025_crypto.csv' => report }
    refused = { '1_US_2025_crypto.csv.refused.json' => Pages::ApiGrid::TAX_TOKENIZED }
    us = { country: 'US', year: 2025 }
    generate = ->(args = us) { McpParity.tool('generate_tax_report', args) }
    status = ->(args = us) { McpParity.tool('get_tax_report_status', args) }
    download = ->(args = us) { McpParity.tool('download_tax_report', args) }
    at = ->(country, year) { { country:, year: } }
    s['tax_generate'] = tax_ready + [generate.()]
    s['tax_generate_options'] = tax_ready + [generate.(country: ' us ', year: 2025, stablecoin_as_fiat: true, force: false)]
    s['tax_generate_refusals'] = tax_ready + [generate.(at.('XX', 2025)), generate.(at.('US', 2008)), generate.(at.('US', 2101)), generate.(at.('US', 2025.5)),
                                              generate.(at.(' ', 2025)), generate.(at.('', 2025)), generate.(at.('U<b>', 2025)), generate.(at.('XX', 2025.5))]
    s['tax_generate_ready'] = tax_ready([], 'tax_fixtures' => ready) + [generate.(), generate.(us.merge(force: false)), status.(), download.(),
                                                                         generate.(us.merge(force: true))]
    s['tax_generate_generating'] = tax_ready([], 'tax_generating' => true) + [status.(), generate.(), generate.(us.merge(force: true)), status.(at.('US', 2024)),
                                                                             download.(), generate.(at.('XX', 2025))]
    s['tax_generate_generating_ready'] = tax_ready([], 'tax_generating' => true, 'tax_fixtures' => ready) + [generate.(), status.(), download.()]
    s['tax_generate_refused'] = tax_ready([], 'tax_fixtures' => refused) + [status.(), download.(), generate.()]
    s['tax_generate_ready_and_refused'] = tax_ready([], 'tax_fixtures' => ready.merge(refused)) + [status.(), download.(), generate.(), generate.(us.merge(force: true))]
    s['tax_market_data_off'] = tax_ready([], 'tax_market_data' => 'none') + [generate.(), status.(), download.()]
    s['tax_status'] = tax_ready([], 'tax_fixtures' => files) + [status.(at.('US', 2024)), status.(at.('us', 2024)), status.(at.('US', 2023)), status.(at.('US', 2022)),
                                                                status.(at.('US', 2021)), status.(), status.(at.('DE', 2025)), status.(at.('XX', 2025)),
                                                                status.(at.('US', 2008)), status.(at.('US', 2024.5)), status.(at.('US', 2024.0))]
    s['tax_download'] = tax_ready([], 'tax_fixtures' => files) + [download.(at.('US', 2024)), download.(at.('us', 2024)), download.(at.('US', 2023)),
                                                                  download.(at.('US', 2022)), download.(at.('US', 2021)), download.(), download.(at.('XX', 2024)),
                                                                  download.(at.('US', 2101)), download.(at.('US', 2024))]
    [generate.({}), generate.(country: 'US'), generate.(country: 'US', year: '2025'), generate.(country: 1, year: 2025),
     generate.(us.merge(force: 'true')), generate.(us.merge(stablecoin_as_fiat: 1)), generate.(us.merge(report_scope: 'broker')),
     status.(country: 'US'), status.(at.('US', '2025')), download.(year: 2025), download.(us.merge(extra: true))].each_with_index do |step, i|
      s["tax_schema_#{i}"] = tax_ready + [step]
    end
    s['tax_not_granted'] = tax_ready(["UPDATE connected_clients SET mcp_tools=#{ActiveRecord::Base.connection.quote(NAMES.to_json)}"], 'tax_fixtures' => ready) +
                           [generate.(), status.(), download.()]
    s['tax_disabled'] = tax_ready(["UPDATE users SET mcp_settings=json_set(mcp_settings,#{TAX_NAMES.map { |n| "'$.tool_permissions.#{n}',json('false')" }.join(',')})"],
                                  'tax_fixtures' => ready) + [generate.(), status.(), download.()]
    s['tax_paper'] = tax_ready(["UPDATE users SET mcp_settings=json_set(mcp_settings,'$.dry_run',json('true')) WHERE email='owner@example.com'"], 'tax_fixtures' => ready) +
                     [status.(), download.(), generate.(us.merge(force: true))]
    # D-TAX-NOT-PORTED-GATE and the CoinGecko gate (tests/mcp_control.rs tax_difference): validation still answers first.
    # FR is not ported until A12-6; DE is since A12-2 (started last: this build's request row would answer a later status).
    s['tax_d_not_ported'] = tax_ready + [generate.(at.('DE', 2008)), generate.(at.('fr', 2025)), status.(at.('FR', 2025)), generate.(at.('de', 2025))]
    s['tax_d_not_ported_wealth'] = tax_ready([], 'tax_market_data' => 'none') + [generate.(), generate.(at.('NL', 2025)), status.(at.('NL', 2025))]
    s['tax_d_coingecko'] = tax_ready([], 'tax_market_data' => 'coingecko') + [generate.(at.('US', 2008)), generate.(), status.()]
  end

  def grid(root)
    raise 'M4 root must be empty' if Dir.exist?(root) && !Dir.empty?(root)

    FileUtils.mkdir_p(root)
    template = Pages.template(root)
    seed(template)
    scenarios.each do |name, steps|
      dir = File.join(root, name)
      FileUtils.mkdir_p(dir)
      %w[production.sqlite3 production_queue.sqlite3 secret_key_base].each { |f| FileUtils.cp(File.join(template, f), File.join(dir, f)) }
      File.write(File.join(dir, 'steps.json'), JSON.pretty_generate(steps))
      install = steps.first.slice('tax_fixtures', 'tax_generating', 'tax_market_data')
      tax_install(dir, install) unless install.empty?
    end
    puts "#{scenarios.size} M4 scenarios"
  end

  def snapshot
    TABLES.to_h do |t|
      rows = ActiveRecord::Base.connection.select_all("SELECT * FROM #{t} ORDER BY id").to_a
      rows.each do |row|
        JSON_COLUMNS.fetch(t, []).each { |col| row[col] = JSON.parse(row[col]) if row[col].is_a?(String) }
        row.transform_values! { |v| v.is_a?(Float) ? { 'float_bits' => [v].pack('G').unpack1('H*') } : v }
      end
      [t, rows]
    end.tap do |rows|
      # PLAN-A12: the reports asked for (Tax::GenerateReportJob's arguments), as (user, country, year, stablecoin, scope).
      asked = ActiveJob::Base.queue_adapter.respond_to?(:enqueued_jobs) ? ActiveJob::Base.queue_adapter.enqueued_jobs : []
      asked = asked.select { |job| job[:job] == Tax::GenerateReportJob }.map { |job| job[:args].first(4) + [job[:args][4] || 'crypto'] }
      rows['tax_jobs'] = asked.sort_by(&:to_json) if asked.any?
    end
  end

  def record(root)
    # Exercise production key validation; Rails test mode otherwise substitutes a correct dummy key.
    Rails.configuration.dry_run = false
    Pages.singleton_class.define_method(:connect) do |dir|
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
      SolidQueue::Record.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production_queue.sqlite3'))
    end
    McpParity.singleton_class.define_method(:snapshot) { McpControl.snapshot }
    # A standalone dry-run market order reads the touch first (Exchange::Dryable#set_dry_market_order); the price is not shown.
    WebMock.stub_request(:any, /data\.alpaca\.markets/).to_return(status: 503, body: '{"message":"unavailable"}')
    Dir[File.join(root, '*/steps.json')].each do |p|
      ENV.store('APP_TMP_DIR', File.join(File.dirname(p), 'tmp')) # AppPaths.tmp (the tax reports), as tests/common/web.rs sets it
      McpParity.play(File.dirname(p), JSON.parse(File.read(p)))
    end
  end

  def numbers(path)
    values = [nil, true, false, {}, [], 0, 1, 1.5, 1.0000000000000002, 0.00005, 1e15,
              0.000000000000000001, 0.0000000000000000001, '001.20', '1e2', '+1', '-1', '1.', '.1', ' 12.5 ', 'NaN',
              '123456789012345', '1234567890123456', '0.123456789012345678', '0.1234567890123456789']
    vectors = values.map do |v|
      value = v.is_a?(Numeric) ? v.to_f : v # MCP number properties are cast to Float.
      { value: v, decimal: BotApi::Number.parse(value)&.to_s('F') }
    end
    # Oj's ordinary output drops the sign; keep this wire scalar explicitly.
    vectors << { wire: '-0.0', decimal: BotApi::Number.parse(JSON.parse('-0.0'))&.to_s('F') }
    File.write(path, "#{JSON.pretty_generate(vectors)}\n")
  end
end
command, root = ARGV
case command
when 'grid', 'record', 'numbers' then McpControl.public_send(command, root)
else raise 'grid|record <scratch path>'
end
