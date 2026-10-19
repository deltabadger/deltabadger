# Existing-bot HTTP cases and their write snapshots. Loaded by pages.rb.
module Pages
  ACTION_TABLES = %w[bots bot_index_assets bot_activity_logs transactions api_keys users].freeze
  ACTION_JSON = {
    'bots' => %w[settings transient_data],
    'bot_activity_logs' => %w[details],
    'transactions' => %w[error_messages]
  }.freeze

  module_function

  def action_rows
    ACTION_TABLES.to_h do |table|
      rows = ActiveRecord::Base.connection.select_all("SELECT * FROM #{table} ORDER BY id").to_a
      rows.each do |row|
        ACTION_JSON.fetch(table, []).each do |column|
          row[column] = JSON.parse(row[column]) if row[column].is_a?(String)
        end
        decrypted!(table, row)
      end
      [table, rows]
    end
  end

  def action_request(action, form, kind, extra)
    path, method = case action
                   when 'update' then ['/bots/1', 'PATCH']
                   when 'delete' then ['/bots/1/delete', 'DELETE']
                   when 'archive' then ['/bots/1/archive', 'POST']
                   when 'unarchive' then ['/bots/1/archive', 'DELETE']
                   when 'modal' then ['/bots/1/start/edit', 'GET']
                   when 'rename_modal' then ['/bots/1/edit', 'GET']
                   when 'delete_modal' then ['/bots/1/delete/edit', 'GET']
                   when 'archive_modal' then ['/bots/1/archive/edit', 'GET']
                   when 'reverse' then ['/bots/1/reverse', 'POST']
                   else ["/bots/1/#{action}", 'PATCH']
                   end
    root = kind == 'index' ? 'bots_dca_index' : 'bots_dca_multi_asset'
    fields = {}
    if action == 'update' && !extra['empty_root']
      form.each do |key, value|
        if value.is_a?(Hash)
          value.each { |id, weight| fields["#{root}[#{key}][#{id}]"] = weight }
        else
          fields["#{root}[#{key}]"] = value
        end
      end
    end
    headers = TURBO.merge('Referer' => 'http://localhost:3000/bots/1')
    headers['Accept'] = 'text/html' if extra['html']
    headers['Accept'] = 'text/html' if action == 'modal' || action.end_with?('_modal')
    { 'method' => method, 'path' => path, 'form' => fields, 'csrf' => 'header',
      'headers' => headers, 'action_snapshot' => true }.merge(extra.slice('raw_rows', 'raw_clock'))
  end
end

module Pages
  module_function

  def action_cases
    cases=[]
    add = ->(name, action, form={}, spec={}, extra={}) { cases << [name,action,form,spec,extra] }
    %w[single basket index].each do |kind|
      add.call("#{kind}_amount", 'update', {'quote_amount'=>'37.25'}, {'kind'=>kind})
      add.call("#{kind}_invalid", 'update', {'quote_amount'=>'0'}, {'kind'=>kind})
      add.call("#{kind}_label", 'update', {'label'=>'A <b>&"'}, {'kind'=>kind})
      add.call("#{kind}_start", 'start?start_fresh=true', {}, {'kind'=>kind})
    end
    %w[created stopped scheduled executing waiting retrying archived deleted].each do |status|
      %w[start?start_fresh=true stop delete archive unarchive modal].each do |act|
        add.call("#{act.split('?')[0]}_#{status}", act, {}, {'columns'=>{'status'=>status}})
      end
    end
    add.call('start_missing', 'start')
    add.call('start_junk', 'start?start_fresh=junk')
    add.call('start_junk_working', 'start?start_fresh=junk', {}, {'columns'=>{'status'=>'scheduled'}})
    add.call('start_no_key', 'start?start_fresh=true', {}, {}, {'no_key'=>true})
    add.call('start_invalid', 'start?start_fresh=true', {}, {'stored'=>{'quote_amount'=>0}})
    add.call('stop_invalid', 'stop', {}, {'stored'=>{'quote_amount'=>0},'columns'=>{'status'=>'scheduled'}})
    add.call('archive_invalid', 'archive', {}, {'stored'=>{'quote_amount'=>0}})
    add.call('start_future','start?start_fresh=true',{}, {'settings'=>{'start_time_enabled'=>true,'start_time_mode'=>'date','start_at'=>'2026-09-12T10:00:00Z'}})
    base_restart={'columns'=>{'status'=>'stopped','started_at'=>'2026-09-10T11:00:00Z'},'transient'=>{'last_action_job_at'=>'2026-09-10T11:00:01Z'}}
    %w[modal start?start_fresh=false start?start_fresh=true].each do |act|
      add.call("#{act.split('?')[0]}_missed_#{act.split('=').last}",act,{},base_restart)
      add.call("#{act.split('?')[0]}_within_#{act.split('=').last}",act,{},base_restart,{'paid'=>true})
    end
    {
      'working_amount'=>{'quote_amount'=>'80'},'working_interval'=>{'interval'=>'week'},
      'working_limit'=>{'limit_ordered'=>'0','limit_order_pcnt_distance'=>'4'},
      'working_smart'=>{'smart_intervaled'=>'1','smart_interval_quote_amount'=>'10'},
      'working_weights'=>{'allocations'=>{'2'=>'0.8','3'=>'0.2'}},
      'working_weighting'=>{'weighting'=>'market_cap'}
    }.each { |n,f| add.call(n,'update',f,{'kind'=>'basket','columns'=>{'status'=>'scheduled','started_at'=>'2026-09-09T12:00:00Z'}}) }
    {
      'blank'=>{'quote_amount'=>'','label'=>'','interval'=>''}, 'junk'=>{'quote_amount'=>' 12.5abc'},
      'bad_interval'=>{'interval'=>'bad'}, 'smart_low'=>{'smart_intervaled'=>'1','smart_interval_quote_amount'=>'0.001'},
      'limit_high'=>{'limit_ordered'=>'true','limit_order_pcnt_distance'=>'101'},
      'cap_on'=>{'quote_amount_limited'=>'1','quote_amount_limit'=>'200'},
      'time_bad'=>{'start_time_enabled'=>'1','start_time_mode'=>'date','start_at'=>'nonsense'},
      'allocations'=>{'allocations'=>{'2'=>'20','3'=>'80'}},
      'normalize'=>{'allocations'=>{'2'=>'20','3'=>'20'},'normalize_allocations'=>'true'},
      'remove'=>{'remove_asset_id'=>'3'},
      'quote_bad'=>{'quote_asset_id'=>'99999'}, 'exchange_bad'=>{'exchange_id'=>'99999'},
      'exchange_ibkr'=>{'exchange_id'=>'2'},'trigger'=>{'price_limited'=>'1','price_limit_mode'=>'above','price_limit'=>'100'},
      'unknown'=>{'status'=>'3','user_id'=>'100','direction'=>'selling','nonsense'=>'foo'}
    }.each { |n,f| add.call("update_#{n}",'update',f,{'kind'=>'basket'}) }
    # Market-cap weights (plan CB T5): switched on over capped coins (the save derives the member rows from the caps), off again
    # (the sliders are the rows), on over stocks with no cap (the sliders are kept), and on over a KuCoin basket.
    add.call('market_cap_on', 'update', { 'weighting' => 'market_cap' }, { 'kind' => 'coins' })
    add.call('market_cap_off', 'update', { 'weighting' => 'manual' }, { 'kind' => 'coins', 'settings' => { 'weighting' => 'market_cap' } })
    add.call('market_cap_uncapped', 'update', { 'weighting' => 'market_cap' }, { 'kind' => 'basket' })
    add.call('market_cap_kucoin', 'update', { 'weighting' => 'market_cap' }, { 'kind' => 'kucoin_basket' }, { 'kucoin_bots' => true })
    # A structural edit, held to the stored text of the whole bot and member rows.
    add.call('update_add', 'update', { 'add_asset_id' => '4' }, { 'kind' => 'basket' }, { 'raw_rows' => { 'bots' => '*', 'bot_index_assets' => '*' } })
    add.call('index_unchanged_slider','update',{'num_coins'=>'5','num_coins_rendered'=>'5','num_coins_ceiling'=>'5'},{'kind'=>'index'})
    add.call('index_changed_slider','update',{'num_coins'=>'5','num_coins_rendered'=>'10','num_coins_ceiling'=>'5'},{'kind'=>'index'})
    add.call('index_bad_flat','update',{'allocation_flattening'=>'1.1'},{'kind'=>'index'})
    %w[update start?start_fresh=true stop delete archive unarchive modal].each do |act|
      add.call("foreign_#{act.split('?')[0]}",act,{'quote_amount'=>'20'},{},{'foreign'=>true})
    end
    %w[modal start?start_fresh=false start?start_fresh=true].each do |act|
      add.call("extra_paid_#{act}", act, {}, base_restart, {'paid'=>true, 'wash_off'=>true})
    end
    %w[update start?start_fresh=true stop delete archive unarchive].each do |act|
      add.call("extra_defaults_#{act}",act,{'label'=>'Renamed'},{'without'=>['limit_ordered','smart_interval_quote_amount','quote_amount_limit','price_limit']})
      add.call("extra_html_#{act}",act,{'label'=>'Renamed'},{},{'html'=>true})
      add.call("extra_signed_out_#{act}",act,{'quote_amount'=>'80'},{},{'signed_out'=>true})
    end
    add.call('extra_cap_off','update',{'quote_amount_limited'=>'0'},{'kind'=>'single'})
    add.call('extra_time_blank','update',{'start_at'=>''},{'settings'=>{'start_at'=>'2026-09-12T10:00:00Z'}})
    add.call('extra_empty_root','update',{}, {}, {'empty_root'=>true})
    add.call('extra_readd','update',{'add_asset_id'=>'3'},{},{'exited'=>true})
    add.call('extra_partial_fill','update',{'quote_amount'=>'20'},base_restart,{'partial'=>true,'wash_off'=>true})
    add.call('extra_pending_weights','update',{'allocations'=>{'2'=>'20','3'=>'80'}},{'transient'=>{'rebalance_pending'=>{'phase'=>'buying'}}})
    %w[TRUE false 0 1].each { |v| add.call("extra_start_#{v}","start?start_fresh=#{v}") }
    %w[true TRUE on off false 0 1].each { |v| add.call("extra_boolean_#{v == 'TRUE' ? 'upper_true' : v}",'update',{'limit_ordered'=>v}) }
    add.call('extra_hour','start?start_fresh=true',{}, {'settings'=>{'start_time_enabled'=>true,'start_time_mode'=>'hour','start_time_of_day'=>'13:00'}})
    add.call('extra_date_past','start?start_fresh=true',{}, {'settings'=>{'start_time_enabled'=>true,'start_time_mode'=>'date','start_at'=>'2026-09-09T10:00:00Z'}})

    # Histories the engine reads: the start modal and both starts of a stopped index bot with liquidation, redeploy and
    # rebalance legs, and of a basket that sold.
    { 'index_history' => %w[index index_rotation], 'basket_sold' => %w[basket basket_sold] }.each do |name, (kind, orders)|
      %w[modal start?start_fresh=false start?start_fresh=true].each do |act|
        add.call("#{name}_#{act.split('?')[0]}_#{act.split('=').last}", act, {}, base_restart.merge('kind' => kind, 'orders' => orders), { 'wash_off' => true })
      end
    end

    %w[single basket index].each do |kind|
      %w[rename_modal delete_modal archive_modal].each do |action|
        add.call("extra_form_#{kind}_#{action}", action, {}, { 'kind' => kind })
      end
    end

    # One-asset baskets on Kraken and Binance, in their venues' own quotes.
    %w[kraken binance].each do |kind|
      add.call("venue_#{kind}_amount", 'update', { 'quote_amount' => '37.25' }, { 'kind' => kind },
               { 'raw_rows' => { 'bots' => %w[id settings transient_data] } })
      add.call("venue_#{kind}_modal", 'modal', {}, { 'kind' => kind })
      add.call("venue_#{kind}_start", 'start?start_fresh=true', {}, { 'kind' => kind })
      add.call("venue_#{kind}_stop", 'stop', {}, { 'kind' => kind, 'columns' => { 'status' => 'scheduled' } })
      add.call("venue_#{kind}_archive", 'archive', {}, { 'kind' => kind })
      add.call("venue_#{kind}_delete_modal", 'delete_modal', {}, { 'kind' => kind })
    end

    # ⇄ on a one-asset basket: buying → selling N base → selling for N quote → buying, at work, and with a buy still resting.
    sells_base = { 'direction' => 'selling', 'sell_denomination' => 'base', 'sell_amount' => 2, 'sell_interval' => 'week', 'smart_intervaled' => true,
                   'smart_interval_base_amount' => 0.5 }
    # The owner has answered the wash-sale question: a bot that sells owes it otherwise, and its modal is not served here.
    decided = { 'wash_off' => true }
    add.call('reverse_buying', 'reverse', {}, { 'kind' => 'single', 'columns' => { 'status' => 'stopped' } }, decided)
    add.call('reverse_selling_base', 'reverse', {}, { 'kind' => 'single', 'settings' => sells_base, 'columns' => { 'status' => 'stopped' } }, decided)
    add.call('reverse_selling_quote', 'reverse', {}, { 'kind' => 'single', 'settings' => { 'direction' => 'selling', 'sell_denomination' => 'quote', 'sell_quote_amount' => 40 },
                                                       'columns' => { 'status' => 'stopped' } }, decided)
    # At work, or with an order to cancel, the install's guard reads the bot: on one coin (16, BTC), as the stock listings of this
    # grid carry no instrument type it accepts.
    coin = { 'kind' => 'coins', 'settings' => { 'allocations' => { '16' => 1.0 } } }
    add.call('reverse_scheduled', 'reverse', {}, coin.merge('columns' => { 'status' => 'scheduled', 'started_at' => '2026-09-09T12:00:00Z' }), decided)
    add.call('reverse_waiting_buy', 'reverse', {}, base_restart.merge(coin).merge('orders' => 'open_coin'), decided)

    # The sell sentence and its split saved on a one-asset basket that sells, and the interval a working seller may not change.
    sells_quote = { 'direction' => 'selling', 'sell_denomination' => 'quote', 'sell_quote_amount' => 40 }
    at_rest = { 'columns' => { 'status' => 'stopped' } }
    {
      'amount' => [sells_base, { 'sell_amount' => '3.5' }], 'blank' => [sells_base, { 'sell_amount' => '' }],
      'interval' => [sells_base, { 'sell_interval' => 'month' }], 'split' => [sells_base, { 'smart_interval_base_amount' => '0.25' }],
      'split_low' => [sells_base, { 'smart_interval_base_amount' => '0.0000001' }], 'quote' => [sells_quote, { 'sell_quote_amount' => '55.5' }],
      'zero' => [sells_base, { 'sell_amount' => '0' }], 'quote_negative' => [sells_quote, { 'sell_quote_amount' => '-1' }]
    }.each { |n, (settings, form)| add.call("sell_#{n}", 'update', form, at_rest.merge('kind' => 'single', 'settings' => settings), decided) }
    add.call('sell_working_interval', 'update', { 'sell_interval' => 'month' },
             { 'kind' => 'single', 'settings' => sells_base, 'columns' => { 'status' => 'scheduled', 'started_at' => '2026-09-09T12:00:00Z' } }, decided)

    # ⇄ and the sell sentence on Kraken and Binance one-asset baskets, stopped and at work.
    %w[kraken binance].each do |kind|
      venue_sells = { 'direction' => 'selling', 'sell_denomination' => 'base', 'sell_amount' => 0.002, 'sell_interval' => 'week' }
      add.call("venue_#{kind}_reverse_buying", 'reverse', {}, { 'kind' => kind, 'columns' => { 'status' => 'stopped' } }, decided)
      add.call("venue_#{kind}_reverse_selling_base", 'reverse', {}, { 'kind' => kind, 'settings' => venue_sells, 'columns' => { 'status' => 'stopped' } }, decided)
      add.call("venue_#{kind}_reverse_selling_quote", 'reverse', {}, { 'kind' => kind, 'settings' => { 'direction' => 'selling', 'sell_denomination' => 'quote', 'sell_quote_amount' => 30 },
                                                                       'columns' => { 'status' => 'stopped' } }, decided)
      add.call("venue_#{kind}_reverse_scheduled", 'reverse', {}, { 'kind' => kind, 'columns' => { 'status' => 'scheduled', 'started_at' => '2026-09-09T12:00:00Z' } }, decided)
      add.call("venue_#{kind}_sell_amount", 'update', { 'sell_amount' => '0.0035' }, { 'kind' => kind, 'settings' => venue_sells, 'columns' => { 'status' => 'stopped' } }, decided)
    end

    # Baskets of two on Kraken (bitcoin, ether in EUR) and Binance (ether, solana in USDT): the sliders, the structural edits, a move
    # to the other venue, the sentence, the lifecycle and its modals. Asset ids as the fixtures create them: 16 BTC, 17 ETH, 20 SOL;
    # exchanges 3 Kraken, 4 Binance. The amount limit is refused on both venues, so no limit edit is made here; Start on a capped
    # basket is the guard's refusal, and ⇄ flips a basket of several into selling, which this build does not trade there.
    rows = { 'raw_rows' => { 'bots' => '*', 'bot_index_assets' => '*' } }
    { 'kraken' => [%w[16 17], '20', '4'], 'binance' => [%w[17 20], '16', '3'] }.each do |venue, ((first, second), other, elsewhere)|
      kind = "#{venue}_basket"
      basket = ->(name, action, form = {}, spec = {}, extra = {}) { add.call("venue_basket_#{venue}_#{name}", action, form, { 'kind' => kind }.merge(spec), extra) }
      at_work = { 'columns' => { 'status' => 'scheduled', 'started_at' => '2026-09-09T12:00:00Z' } }
      # Bitcoin and ether in USDT, which both venues list once Kraken carries them in USDT.
      movable = { 'settings' => { 'quote_asset_id' => 19, 'allocations' => { '16' => 0.6, '17' => 0.4 } } }
      basket.('sliders', 'update', { 'allocations' => { first => '70', second => '30' } }, {}, rows)
      basket.('sliders_off', 'update', { 'allocations' => { first => '50', second => '30' } }, {}, rows)
      basket.('normalize', 'update', { 'allocations' => { first => '30', second => '30' }, 'normalize_allocations' => '1' }, {}, rows)
      basket.('add', 'update', { 'add_asset_id' => other }, {}, rows)
      basket.('remove', 'update', { 'allocations' => { first => '60', second => '40' }, 'remove_asset_id' => second }, {}, rows)
      basket.('exchange', 'update', { 'exchange_id' => elsewhere }, movable, rows.merge('kraken_usdt' => true))
      basket.('exchange_unsupported', 'update', { 'exchange_id' => elsewhere }, {}, rows)
      basket.('exchange_working', 'update', { 'exchange_id' => elsewhere }, movable.merge(at_work), rows.merge('kraken_usdt' => true))
      basket.('amount', 'update', { 'quote_amount' => '37.25' }, {}, rows)
      basket.('interval', 'update', { 'interval' => 'month' })
      basket.('limit', 'update', { 'limit_ordered' => '1', 'limit_order_pcnt_distance' => '2' })
      basket.('smart', 'update', { 'smart_intervaled' => '1', 'smart_interval_quote_amount' => '10' })
      basket.('rename', 'update', { 'label' => 'Venue basket' })
      basket.('html', 'update', { 'label' => 'Renamed' }, {}, { 'html' => true })
      basket.('modal', 'modal')
      basket.('start', 'start?start_fresh=true')
      basket.('start_unbalanced', 'start?start_fresh=true', {}, { 'stored' => { 'allocations' => { first => 0.5, second => 0.3 } } })
      basket.('start_capped', 'start?start_fresh=true', {}, { 'settings' => { 'quote_amount_limited' => true, 'quote_amount_limit' => 200 } })
      basket.('stop', 'stop', {}, at_work)
      basket.('archive', 'archive')
      basket.('unarchive', 'unarchive', {}, { 'columns' => { 'status' => 'archived' } })
      basket.('delete_modal', 'delete_modal')
      basket.('delete', 'delete')
      basket.('rename_modal', 'rename_modal')
      basket.('not_ported_reverse', 'reverse', {}, { 'columns' => { 'status' => 'stopped' } }, decided)
    end

    # Plan CB T4: a save on a basket on each crypto venue whose history holds a settled swap (the engine reads its REBALANCE
    # rows now, so the settings it saves are this build's).
    %w[kraken binance kucoin].each do |venue|
      add.call("venue_rebalance_history_#{venue}_rename", 'update', { 'label' => 'Rebalanced' }, { 'kind' => "#{venue}_basket", 'orders' => 'venue_rebalance_history' },
               { 'wash_off' => true }.merge(venue == 'kucoin' ? { 'kucoin_bots' => true } : {}))
    end

    raise "expected 254 bot action cases" unless cases.size == 254
    names = cases.map { |entry| "actions_#{entry.first}".gsub(/[^a-zA-Z0-9_-]/, "_") }
    raise "action case names collide after filesystem conversion" unless names.uniq.size == names.size
    cases
  end
end

module Pages
  module_function

  ACTION_EXCEPTIONS = {
    'actions_extra_html_archive' => 406,
    'actions_extra_html_update' => 406, 'actions_extra_html_start_start_fresh_true' => 406,
    'actions_extra_html_stop' => 406, 'actions_extra_empty_root' => 400,
    'actions_venue_basket_kraken_html' => 406, 'actions_venue_basket_binance_html' => 406
  }.freeze

  # Enumerate the actual primary schema, including tables without an id (schema_migrations).
  # The queue is a separate database: its job descriptions are evidence, not primary-row parity.
  def action_other_rows
    connection = ActiveRecord::Base.connection
    (connection.tables - ACTION_TABLES).reject { |table| table.start_with?('sqlite_') }.sort.to_h do |table|
      columns = connection.columns(table).map { |column| connection.quote_column_name(column.name) }
      [table, connection.select_all("SELECT * FROM #{connection.quote_table_name(table)} ORDER BY #{columns.join(', ')}").to_a]
    end
  end

  def action_scenarios
    action_cases.to_h do |name, action, form, spec, extra|
      name = "actions_#{name}".gsub(/[^a-zA-Z0-9_-]/, '_')
      request = action_request(action, form, spec.fetch('kind', 'basket'), extra)
      request['client'] = 'signed_out' if extra['signed_out']
      request['rails_exception_status'] = ACTION_EXCEPTIONS[name] if ACTION_EXCEPTIONS.key?(name)
      scenario = {
        'install' => 'alpaca', 'user' => owner('wash_sale_enabled' => nil),
        'bots' => [{ 'kind' => 'basket' }.merge(spec), { 'kind' => 'basket', 'columns' => { 'status' => 7 } }],
        'action_extra' => extra,
        'steps' => signed_in(get('/bots/2').merge('expect' => 200), request)
      }
      if action.end_with?('_modal')
        scenario['steps'] += [request.deep_dup.tap { |r| r['headers']['Turbo-Frame'] = 'modal' }]
        [false, true].each do |frame|
          scenario['steps'] << request.deep_dup.tap do |r|
            r['path'] = "/de#{r['path']}"
            r['headers']['Turbo-Frame'] = 'modal' if frame
          end
        end
      end
      scenario['extra_users'] = [user('email' => 'other@example.com', 'admin' => false)] if extra['foreign']
      scenario['venues'] = true if VENUE_KINDS.key?(spec['kind'])
      scenario['kraken_usdt'] = true if extra['kraken_usdt']
      scenario['kucoin_bots'] = true if extra['kucoin_bots']
      [name, scenario]
    end.tap { |grid| raise 'action case names collide' unless grid.size == 254 }
  end
end

# Locale variants are independent installs within the same named scenario. Replaying a Start or
# Delete on its already-mutated row would test a different action rather than its translation.
module Pages
  class << self
    alias_method :grid_without_action_locales, :grid
    alias_method :record_without_action_locales, :record

    def grid(root)
      raise 'empty page selection' if selected.empty?
      grid_without_action_locales(root)
      crypto_names = %w[single_amount single_start start_missed_false start_missed_true]
      scratch_template = template(root)
      selected.each do |name, original|
        next unless crypto_names.include?(name.delete_prefix('actions_'))

        spec = original.deep_dup
        spec['user']['wash_sale_enabled'] = false
        spec['bots'][0]['kind'] = 'coins'
        spec['bots'][0]['settings'] = { 'allocations' => { '16' => 1.0 }, 'interval' => 'day' }
        spec['bots'][0]['columns'] ||= {}
        spec['bots'][0]['columns']['status'] = 'stopped'
        %w[crypto crypto_de].each do |variant|
          dir = File.join(root, name, variant)
          jobs = build(dir, scratch_template, spec)
          steps = spec['steps'].deep_dup
          steps.each { |step| step['path'] = "/de#{step['path']}" if step['action_snapshot'] } if variant == 'crypto_de'
          File.write(File.join(dir, 'scenario.json'), JSON.pretty_generate(
            'page_parity_scratch' => true, 'at' => AT, 'secret_key_base' => Rails.application.secret_key_base,
            'steps' => steps, 'jobs' => jobs, 'divergence' => 'crypto_actions'
          ))
        end
      end
      FileUtils.rm_rf(scratch_template)
      Dir[File.join(root, 'actions_*/scenario.json')].each do |path|
        dir = File.dirname(path)
        localized = File.join(dir, 'de')
        FileUtils.mkdir_p(localized)
        %w[production.sqlite3 production_queue.sqlite3].each { |file| FileUtils.cp(File.join(dir, file), localized) }
        scenario = JSON.parse(File.read(path))
        scenario['steps'].each do |step|
          next unless step['action_snapshot']

          step['path'] = "/de#{step['path']}" unless step['path'].start_with?('/de/')
        end
        File.write(File.join(localized, 'scenario.json'), JSON.pretty_generate(scenario))
      end
    end

    def record(root)
      # ⇄ with a buy resting (actions_reverse_waiting_buy): Alpaca accepts the cancel, and the re-read of the order fails, so
      # Rails leaves its reserve to the order's final poll (flip_reserve_pending), as it does for a cancel still pending.
      order = 'https://paper-api.alpaca.markets/v2/orders/order-1-BTC-2026-09-10T11:00:00Z'
      WebMock.stub_request(:delete, order).to_return(status: 204, body: '')
      WebMock.stub_request(:get, order).to_return(status: 503, body: '{"message":"unavailable"}', headers: { 'Content-Type' => 'application/json' })
      record_without_action_locales(root)
      Dir[File.join(root, 'actions_*')].select { |path| File.directory?(path) }.each do |dir|
        record_without_action_locales(dir)
      end
    end
  end
end
