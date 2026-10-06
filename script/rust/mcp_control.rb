# Synthetic M4 recorder; Rails application code remains the oracle, unchanged.
source = Rails.root.join('script/rust/mcp.rb').read.split("command, root = ARGV\ncase command\n").first
raise 'M2 recorder boundary changed' unless source.include?('module McpParity')

# The pinned, local recorder source supplies shared fixtures; no external code is evaluated.
eval(source, TOPLEVEL_BINDING, Rails.root.join('script/rust/mcp.rb').to_s) # rubocop:disable Security/Eval
module McpControl
  extend ActiveSupport::Testing::TimeHelpers

  module_function

  NAMES = %w[stop_bot archive_bot unarchive_bot delete_bot update_bot_settings start_bot].freeze
  TABLES = (McpParity::TABLES + %w[bot_index_assets bot_activity_logs users]).uniq.freeze
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
      allowed = ENV.key?('M4_TOOLS') ? ENV.fetch('M4_TOOLS').split(',') : NAMES
      app = Doorkeeper::Application.create!(name: 'M4', uid: 'm2-client', redirect_uri: 'http://localhost/cb', confidential: false, scopes: 'mcp api')
      { 'm2-token' => [owner, 'mcp'], 'api-token' => [owner, 'api'], 'second-token' => [second, 'mcp'] }.each do |token, (user, scopes)|
        Doorkeeper::AccessToken.create!(application: app, resource_owner_id: user.id, scopes:, expires_in: 3600).update_columns(token:)
        ConnectedClient.find_or_create_by!(user:, oauth_application: app).update!(mcp_tools: allowed)
      end
      owner.update_columns(mcp_settings: { tool_permissions: NAMES.to_h { |n| [n, true] }, dry_run: true })
      second.update_columns(mcp_settings: { tool_permissions: NAMES.to_h { |n| [n, true] } })
      Pages.bot('kind' => 'coins', 'columns' => { 'status' => 'stopped', 'label' => 'Control' })
      Pages.bot('kind' => 'index', 'columns' => { 'status' => 'stopped', 'label' => 'Index' })
      Bot.update_all(updated_at: Time.iso8601('2026-01-01T00:00:00Z'))
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
    allowed = ENV.key?('M4_TOOLS') ? ENV.fetch('M4_TOOLS').split(',') : NAMES
    reads = %w[list_bots get_bot_details list_exchanges get_exchange_balances get_portfolio_summary list_transactions
               list_open_orders list_tax_jurisdictions]
    registered = AppConfig::MCP_TOOL_DEFAULTS.keys.select { |name| reads.include?(name) || allowed.include?(name) }
    grant = "UPDATE connected_clients SET mcp_tools=#{ActiveRecord::Base.connection.quote(registered.to_json)}"
    s['registry'] = McpParity.ready + [McpParity.session_rpc('tools/list').merge('sql' => [grant])]
    [0, 10, 20].each do |offset|
      s['registry'] << McpParity.session_rpc('tools/list', { cursor: Base64.urlsafe_encode64(offset.to_s, padding: false) })
    end
    s['registry'] += [McpParity.tool('create_bot'), McpParity.tool('create_index_bot'), McpParity.tool('create_signal_bot')]
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
    s['start_continue_empty'] = McpParity.ready + [tool('start_bot', {}, [set_status(2)])]
    s['start_continue_within'] = McpParity.ready + [tool('start_bot', {}, [set_status(2), "UPDATE bots SET started_at='2026-09-10 12:00:00', transient_data=json_set(transient_data,'$.last_action_job_at','2026-09-10T12:00:01Z') WHERE id=1", # rubocop:disable Layout/LineLength -- Literal transcript data keeps request values visible together.
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
    prefixes = ENV.fetch('M4', '').split(',')
    selected = s.select { |name, _| prefixes.empty? || prefixes.any? { |p| name.start_with?(p) } }
    raise 'empty M4 scenario filter' if selected.empty?

    selected
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
    end
  end

  def record(root)
    Pages.singleton_class.define_method(:connect) do |dir|
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
      SolidQueue::Record.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production_queue.sqlite3'))
    end
    McpParity.singleton_class.define_method(:snapshot) { McpControl.snapshot }
    Dir[File.join(root, '*/steps.json')].each { |p| McpParity.play(File.dirname(p), JSON.parse(File.read(p))) }
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
