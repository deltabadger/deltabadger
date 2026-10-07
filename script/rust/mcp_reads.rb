# Only the recorder changes: the tools and BotApi services remain the oracle.
FIGURES_LIBRARY = true
require_relative 'figures'
module McpReads
  module_function
  NAMES = %w[get_exchange_balances list_open_orders get_bot_details get_portfolio_summary].freeze
  def scenarios
    source = Figures.scenarios
    basket = source.find { |s| s['name'] == 'basket_buys' }['bots'].first.deep_dup
    single = source.find { |s| s['name'] == 'row_readings' }['bots'].first.deep_dup
    index = source.find { |s| s['name'] == 'index_rotation' }.deep_dup
    index.delete('delist')
    index['bots'].first['orders'].reject! { |o| o['sym'] == 'LLL' }
    index['bots'].first['exited'].delete('LLL')
    basket['label'] = 'Basket'
    basket['settings'] = { 'limit_ordered' => true, 'smart_intervaled' => true, 'smart_interval_quote_amount' => 10.0 }
    # The owner control has priced fills; NULL-price executions have isolated refusal scenarios.
    single['orders'].reject! { |o| o['price'].nil? || o['price'].to_f <= 0 }
    single['orders'].each do |order|
      quantity = order['amount_exec'] || (order.fetch('ext', 'closed') == 'closed' ? order['amount'] : nil)
      order['quote_amount_exec'] = (order['price'].to_d * quantity.to_d).to_s('F') if quantity.to_d.positive? && !order['quote_amount_exec'].to_d.positive?
    end
    single['label'] = 'Limited'
    single['orders'].each { |o| o['order_type'] = 'limit_order' if o['ext'] == 'open' }
    single['settings'].merge!('limit_ordered' => true, 'quote_amount_limited' => true, 'quote_amount_limit' => 1000)
    index['bots'].first['label'] = 'ND100'
    index['bots'] = [basket, single, index['bots'].first]
    index['at'] = McpParity::AT
    index['provider'] = 'deltabadger'
    index['script']['GET paper-api.alpaca.markets/v2/account'] = Figures.ok('cash' => '123.45')
    index['script']['GET paper-api.alpaca.markets/v2/positions'] = Figures.ok([{ 'symbol' => 'AAA', 'asset_class' => 'us_equity', 'qty' => '2.5' }])
    index['script']['GET paper-api.alpaca.markets/v2/orders?limit=50&status=open'] = Figures.ok([
      { 'id' => 'external-limit', 'symbol' => 'AAA', 'asset_class' => 'us_equity', 'status' => 'new', 'side' => 'buy', 'type' => 'limit', 'qty' => '2', 'limit_price' => '100', 'filled_qty' => '0' }
    ])
    calls = [McpParity.tool('get_exchange_balances', exchange_name: 'Alpaca'), McpParity.tool('list_open_orders'),
             *[1,2,3].map { |id| McpParity.tool('get_bot_details', bot_id: id) }, McpParity.tool('get_portfolio_summary')]
    index['script']['POST api.kraken.com/0/private/BalanceEx'] = Figures.ok('error' => ['EAPI:Invalid key'])
    index['script']['GET api.kraken.com/0/public/Ticker'] = Figures.ok('error' => ['EAPI:Unavailable'])
    result = { 'm3_owner' => [index, McpParity.ready + calls] }
    result['m3_not_granted'] = [index, McpParity.ready + NAMES.map { |name| McpParity.tool(name).merge('sql' => ["UPDATE connected_clients SET mcp_tools='[]'"]) }]
    result['m3_unknown_bot'] = [index, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 999), McpParity.tool('get_bot_details', bot_id: 4)]]
    result['m3_missing_key'] = [index, McpParity.ready + calls.first(2).map { |s| s.merge('sql' => ['DELETE FROM api_keys']) }]
    identity = index.deep_dup
    identity['script']['GET paper-api.alpaca.markets/v2/orders?limit=50&status=open']['body'] = [
      { 'id' => 'wrong-class', 'symbol' => 'AAA', 'asset_class' => 'crypto', 'status' => 'new', 'side' => 'buy', 'type' => 'limit', 'qty' => '2', 'limit_price' => '100', 'filled_qty' => '0' },
      { 'id' => 'unknown', 'symbol' => 'UNKNOWN', 'asset_class' => 'us_equity', 'status' => 'new', 'side' => 'buy', 'type' => 'limit', 'qty' => '2', 'limit_price' => '100', 'filled_qty' => '0' },
      { 'id' => 'known-tombstone', 'symbol' => 'AAA', 'asset_class' => 'us_equity', 'status' => 'new', 'side' => 'buy', 'type' => 'limit', 'qty' => '2', 'limit_price' => '100', 'filled_qty' => '0' }
    ]
    result['m3_order_identity'] = [identity, McpParity.ready + [McpParity.tool('list_open_orders').merge('sql' => [
      "UPDATE tickers SET ticker='__stale_' || id || '_' || ticker,base='__stale_' || id || '_' || base,available=0 WHERE ticker='AAA'"
    ])]]
    %w[status number missing].each do |defect|
      malformed = identity.deep_dup
      bad = malformed['script']['GET paper-api.alpaca.markets/v2/orders?limit=50&status=open']['body'][1]
      bad['status'] = 'future_status' if defect == 'status'
      bad['filled_qty'] = 'unreadable' if defect == 'number'
      bad.delete('filled_qty') if defect == 'missing'
      result["m3_order_identity_#{defect}"] = [malformed, result['m3_order_identity'][1].deep_dup]
    end
    bare = identity.deep_dup
    path = 'GET paper-api.alpaca.markets/v2/orders?limit=50&status=open'
    bare['script'][path]['body'] = bare['script'][path]['body'].to_json.sub('"filled_qty":"0"', '"filled_qty":1e-350')
    result['m3_order_identity_bare_number'] = [bare, result['m3_order_identity'][1].deep_dup]
    result['m3_unknown_exchange'] = [index, McpParity.ready + %w[get_exchange_balances list_open_orders].map { |n| McpParity.tool(n, exchange_name: 'NoVenue') }]
    result['m3_other_venue'] = [index, McpParity.ready + [
      McpParity.tool('get_exchange_balances', exchange_name: 'Kraken').merge('sql' => ["UPDATE exchanges SET name='Kraken',type='Exchanges::Kraken' WHERE id=1"]),
      McpParity.tool('list_open_orders', exchange_name: 'Kraken')]]
    failed = index.deep_dup
    %w[account positions orders?limit=50&status=open].each { |p| failed['script']["GET paper-api.alpaca.markets/v2/#{p}"] = { 'status' => 401, 'body' => { 'message' => 'unauthorized' } } }
    result['m3_venue_failure'] = [failed, McpParity.ready + calls.first(2)]
    unpriced = index.deep_dup
    unpriced['script'][Figures::STOCK_PRICES]['body'].delete('AAA')
    result['m3_unpriced'] = [unpriced, McpParity.ready + [calls.last]]
    result['m3_other_venue_figures'] = [index, McpParity.ready + [
      McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => ["UPDATE exchanges SET name='Kraken',type='Exchanges::Kraken' WHERE id=1"]),
      McpParity.tool('get_portfolio_summary')]]
    result['m3_other_venue_missing_key'] = [index, McpParity.ready + [
      McpParity.tool('get_exchange_balances', exchange_name: 'Kraken').merge('sql' => ["UPDATE exchanges SET name='Kraken',type='Exchanges::Kraken' WHERE id=1", 'DELETE FROM api_keys']),
      McpParity.tool('list_open_orders', exchange_name: 'Kraken')]]
    result['m3_empty'] = [index, McpParity.ready + [McpParity.tool('get_portfolio_summary').merge('sql' => ['DELETE FROM transactions', 'DELETE FROM bot_index_assets', 'DELETE FROM bots'])]]
    result['m3_no_orders'] = [index, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => ['DELETE FROM transactions']), McpParity.tool('get_portfolio_summary')]]
    result['m3_locked'] = [index, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => [
      "UPDATE users SET wash_sale_enabled=1,wash_sale_jurisdiction='US' WHERE id=1",
      "INSERT INTO wash_sale_locks(user_id,asset_id,buy_locked_until,source,created_at,updated_at) VALUES(1,2,'2026-09-20 00:00:00','ledger','2026-09-01 00:00:00','2026-09-01 00:00:00')"])]]
    zero = index.deep_dup
    zero['script']['GET paper-api.alpaca.markets/v2/account'] = Figures.ok('cash' => '0')
    zero['script']['GET paper-api.alpaca.markets/v2/positions'] = Figures.ok([])
    liquidation = index.deep_dup
    liquidation['bots'].first['orders'] << Figures.sell('AAA', Figures::T0 + 86_400, '110', '0.25', type: 'LIQUIDATION')
    result['m3_stranded_offset'] = [index, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => ['UPDATE bots SET redeploy_declined_offset=100 WHERE id=1'])]]
    result['m3_redeploy'] = [liquidation, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1)]]
    result['m3_zero_balances'] = [zero, McpParity.ready + [calls.first]]
    dedup = index.deep_dup
    dedup['script']['GET paper-api.alpaca.markets/v2/orders?limit=50&status=open']['body'].first['id'] = 'Limited-2'
    result['m3_deduplicate'] = [dedup, McpParity.ready + [calls[1]]]
    result['m3_deleted_bot'] = [index, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => ['UPDATE bots SET status=3 WHERE id=1'])]]
    result['m3_disabled'] = [index, McpParity.ready + NAMES.map { |name| McpParity.tool(name).merge('sql' => ["UPDATE users SET mcp_settings='#{ {tool_permissions: NAMES.to_h { |n| [n, false] }}.to_json }'"]) }]
    pair = index.deep_dup
    pair['bots'].first['type'] = 'single'
    pair['bots'].first['members'] = ['AAA']
    pair['bots'].first['orders'] = [Figures.buy('AAA', Figures::T0, '100', '2'), Figures.sell('AAA', Figures::T0 + 60, '110', '0.5')]
    result['m3_legacy_pair'] = [pair, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1), McpParity.tool('get_portfolio_summary')]]
    result['m3_unmapped_cash'] = [zero.deep_dup.tap { |s| s['script']['GET paper-api.alpaca.markets/v2/account'] = Figures.ok('cash' => '123.45') }, McpParity.ready + [calls.first.merge('sql' => ["DELETE FROM exchange_assets WHERE asset_id=(SELECT id FROM assets WHERE symbol='USD')"])]]
    unmapped = zero.deep_dup
    unmapped['script']['GET paper-api.alpaca.markets/v2/positions'] = Figures.ok([{ 'symbol' => 'UNKNOWN', 'asset_class' => 'us_equity', 'qty' => '2.5' }])
    result['m3_unmapped_position'] = [unmapped, McpParity.ready + [calls.first]]
    %w[market limit].each do |kind|
      no_price = zero.deep_dup
      no_price['script']['GET paper-api.alpaca.markets/v2/orders?limit=50&status=open']['body'] = [{ 'id' => 'no-price', 'symbol' => 'AAA', 'asset_class' => 'us_equity', 'status' => 'new', 'side' => 'buy', 'type' => kind, 'qty' => '2', 'filled_qty' => '0' }]
      result["m3_#{kind}_no_price"] = [no_price, McpParity.ready + [calls[1].merge('sql' => ['DELETE FROM transactions'])]]
    end
    # Persisted polling sentinel: the engine regression exercises apply_in for this state.
    %w[zero null].each do |price_kind|
      stored = price_kind == 'zero' ? '0' : 'NULL'
      setup = ['DELETE FROM transactions',
               "INSERT INTO transactions(id,bot_id,exchange_id,external_id,status,external_status,side,order_type,amount,amount_exec,quote_amount_exec,price,base,quote,created_at,updated_at) VALUES(9001,1,1,'polled-market',0,1,0,0,2,0,0,#{stored},'AAA','USD','2026-09-10 12:00:00','2026-09-10 12:00:00')"]
      sc = zero.deep_dup
      sc['script']['GET paper-api.alpaca.markets/v2/orders?limit=50&status=open'] = Figures.ok([])
      result["m3_stored_#{price_kind}_price"] = [sc, McpParity.ready + [calls[1].merge('sql' => setup), McpParity.tool('list_transactions')]]
    end
    result['m3_inactive_order_ticker'] = [index, McpParity.ready + [calls[1].merge('sql' => ['UPDATE tickers SET available=0,trading_enabled=0'])]]
    result['m3_inactive_pair_ticker'] = [pair.deep_dup.tap { |v| v['bots'] = [v['bots'].first] }, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => ['UPDATE tickers SET available=0,trading_enabled=0']), calls.last]]
    null_fill = pair.deep_dup
    null_fill['bots'] = [null_fill['bots'].first]
    null_fill['bots'].first['orders'] = [Figures.buy('AAA', Figures::T0, '100', '2'), Figures.sell('AAA', Figures::T0 + 60, '110', '1')]
    null_fill['script'][Figures::STOCK_PRICES]['body']['AAA'] = { 'latestTrade' => { 'p' => 120 } }
    result['m3_priced_fill_control'] = [null_fill, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1), calls.last]]
    result['m3_null_fill_price'] = [null_fill, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => ["UPDATE transactions SET price=NULL WHERE bot_id=1 AND side=1"]), calls.last]]
    dust = index.deep_dup
    dust['bots'].first['orders'] << Figures.sell('AAA', Figures::T0 + 86_400, '100', '0.0001', type: 'LIQUIDATION')
    result['m3_disabled_redeploy'] = [dust, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => ['UPDATE tickers SET available=0,trading_enabled=0,minimum_quote_size=1'])]]
    result['m3_empty_redeploy_minimum'] = [dust, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => ['DELETE FROM bot_index_assets WHERE bot_id=1', 'UPDATE tickers SET available=0,trading_enabled=0'])]]
    %w[basket index].each do |kind|
      sc = null_fill.deep_dup
      sc['bots'].first['type'] = kind
      sc['bots'].first['settings'] = {}
      sc['bots'].first.delete('weights')
      result["m3_#{kind}_priced_fill"] = [sc, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1), calls.last]]
      result["m3_#{kind}_null_fill"] = [sc, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => ["UPDATE transactions SET price=NULL WHERE bot_id=1 AND side=1"]), calls.last]]
    end
    # R3: incomplete effective executions, and local non-Alpaca cash-only valuation.
    %w[single basket index signal].each do |kind|
      sc = null_fill.deep_dup
      sc['bots'].first['type'] = kind == 'signal' ? 'single' : kind
      sc['bots'].first['settings'] = {}
      sc['bots'].first.delete('weights')
      typed = kind == 'signal' ? ["UPDATE bots SET type='Bots::Signal' WHERE id=1"] : []
      %w[legacy_buy partial_sell].each do |defect|
        next if defect == 'legacy_buy' && %w[single signal].include?(kind)
        sql = defect == 'legacy_buy' ? "UPDATE transactions SET side=0,price=NULL,amount_exec=NULL,quote_amount_exec=NULL WHERE bot_id=1 AND side=1" : "UPDATE transactions SET external_status=3,price=0,quote_amount_exec=NULL WHERE bot_id=1 AND side=1"
        result["m3_r3_#{kind}_#{defect}"] = [sc, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => typed + [sql]), calls.last]]
      end
      result["m3_r4_#{kind}_known_price"] = [sc, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => typed + ["UPDATE transactions SET external_status=3,quote_amount_exec=NULL WHERE bot_id=1 AND side=1"]), calls.last]]
      { 'null' => 'NULL', 'blank' => "'   '" }.each do |label_kind, stored|
        setup = typed + (kind == 'index' ? ["UPDATE bots SET settings=json_set(settings,'$.num_coins',10) WHERE id=1"] : []) + ["UPDATE bots SET label=#{stored} WHERE id=1"]
        result["m3_r4_#{kind}_#{label_kind}_label"] = [sc, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => setup), calls.last.merge('sql' => setup)]]
      end
      next unless %w[single signal].include?(kind)
      liquidated = sc.deep_dup
      liquidated['bots'].first['orders'].last['amount'] = '2'
      liquidated['bots'].first['orders'].last['amount_exec'] = '2'
      liquidated['bots'].first['orders'].last['quote_amount_exec'] = '220'
      other = typed + ["UPDATE exchanges SET name='Kraken',type='Exchanges::Kraken' WHERE id=1", 'DELETE FROM tickers WHERE exchange_id=1']
      result["m3_r3_#{kind}_liquidated_other"] = [liquidated, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => other), calls.last]]
      result["m3_r3_#{kind}_held_other"] = [sc, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => typed + ["UPDATE exchanges SET name='Kraken',type='Exchanges::Kraken' WHERE id=1"]), calls.last]]
    end
    # R5: generated category labels use Rails' titleized slug and stored count.
    { 'category' => ['layer-1', nil, 20, false], 'whole_category' => ['sp-500', 'S&P 500', 10, true] }.each do |variant, (category, name, count, whole)|
      sc = null_fill.deep_dup
      sc['bots'].first['type'] = 'index'
      sc['bots'].first.delete('weights')
      sc['bots'].first['settings'] = { 'index_category_id' => category, 'index_name' => name, 'num_coins' => count, 'hold_all' => whole }
      metadata = whole ? ["DELETE FROM indices", "INSERT INTO indices(external_id,name,source,top_coins,created_at,updated_at) VALUES('sp-500','S&P 500','deltabadger','#{Array.new(500) { |i| "coin-#{i}" }.to_json}','2026-09-01','2026-09-01')"] : []
      { 'null' => 'NULL', 'blank' => "'   '" }.each do |label_kind, stored|
        setup = metadata + ["UPDATE bots SET label=#{stored} WHERE id=1"]
        result["m3_r5_#{variant}_#{label_kind}_label"] = [sc, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => setup), calls.last.merge('sql' => setup)]]
      end
    end
    missing_quote = pair.deep_dup
    missing_quote['bots'].first['orders'] = [Figures.buy('AAA', Figures::T0, '100', '2'), Figures.sell('AAA', Figures::T0 + 60, '110', '2')]
    result['m3_missing_quote'] = [missing_quote, McpParity.ready + [calls.last.merge('sql' => ["UPDATE bots SET settings=json_set(settings,'$.quote_asset_id',999999) WHERE id=1"])]]
    %w[pair signal].each do |kind|
      sql = ["UPDATE users SET wash_sale_enabled=1,wash_sale_jurisdiction='US' WHERE id=1",
             "INSERT INTO wash_sale_locks(user_id,asset_id,buy_locked_until,source,created_at,updated_at) VALUES(1,2,'2026-09-20 00:00:00','ledger','2026-09-01 00:00:00','2026-09-01 00:00:00')"]
      sql << "UPDATE bots SET type='Bots::Signal' WHERE id=1" if kind == 'signal'
      result["m3_#{kind}_locked"] = [pair, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => sql)]]
    end
    %w[single basket index].each do |kind|
      %w[price price_drop moving_average indicator].each do |trigger|
        %w[buying selling].each do |direction|
          sc = null_fill.deep_dup
          sc['bots'].first['type'] = kind
          sc['bots'].first['settings'] = {}
          sc['bots'].first.delete('weights')
          prefix = direction == 'selling' ? 'sell_' : ''
          # SQL bypasses save callbacks, exactly like the stored-data trigger under review.
          setup = "UPDATE bots SET settings=json_set(settings,'$.direction','#{direction}','$.#{prefix}#{trigger}_limited',json('true')) WHERE id=1"
          met = "UPDATE bots SET transient_data=json_set(transient_data,'$.#{prefix}#{trigger}_limit_condition_met_at','2026-09-09T14:00:00Z') WHERE id=1"
          older = "UPDATE bots SET transient_data=json_set(transient_data,'$.#{prefix}#{trigger}_limit_condition_met_at','2026-01-01T00:00:00Z') WHERE id=1"
          result["m3_start_#{kind}_#{direction}_#{trigger}"] = [sc, McpParity.ready + [setup, met, older].map { |sql| McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => [sql]) }]
        end
      end
    end
    trigger_sql = "UPDATE bots SET settings=json_set(settings,'$.price_limited',json('true'),'$.indicator_limited',json('true')),transient_data=json_set(transient_data,'$.price_limit_condition_met_at','2026-09-09 14:00:00','$.indicator_limit_condition_met_at','2026-09-09T15:00:00Z') WHERE id=1"
    result['m3_start_combined'] = [pair, McpParity.ready + [
      McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => [trigger_sql]),
      McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => ["UPDATE bots SET transient_data=json_remove(transient_data,'$.indicator_limit_condition_met_at') WHERE id=1"]),
      McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => ["UPDATE bots SET settings=json_set(settings,'$.indicator_limited',json('false')) WHERE id=1"]),
      McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => ['UPDATE bots SET started_at=NULL WHERE id=1'])]]
    result['m3_signal_ignores_triggers'] = [pair, McpParity.ready + [McpParity.tool('get_bot_details', bot_id: 1).merge('sql' => [trigger_sql, "UPDATE bots SET type='Bots::Signal' WHERE id=1"])]]
    result['m3_bad_arguments'] = [index, McpParity.ready + [McpParity.tool('get_bot_details'), McpParity.tool('get_bot_details', bot_id: '1'), McpParity.tool('get_exchange_balances'), McpParity.tool('list_open_orders', exchange_name: 1), McpParity.tool('get_portfolio_summary', extra: 1)]]
    result
  end
  def grid(root)
    FileUtils.mkdir_p(root)
    template = Dir.mktmpdir('mcp-reads-world')
    assets = Figures.world(template)
    scenarios.each do |name, (sc, steps)|
      dir = File.join(root, name)
      FileUtils.mkdir_p(dir)
      user, bots = Figures.build(sc.merge('dir' => dir), template, assets)
      ActiveRecord::Base.connection.execute("UPDATE api_keys SET key='M3-key',secret='M3-secret',passphrase='paper'")
      user.update_columns(time_zone: 'Warsaw', wash_sale_enabled: false)
      second = User.new(email: 'second@example.com', password: Pages::PASSWORD, confirmed_at: Time.current)
      second.save!(validate: false)
      bots.first.dup.tap { |bot| bot.user_id = second.id; bot.label = 'Other user'; bot.set_missed_quote_amount; bot.save!(validate: false) }
      app = Doorkeeper::Application.create!(name: 'M3', uid: 'm3-client', redirect_uri: 'http://localhost/cb', confidential: false, scopes: 'mcp')
      Doorkeeper::AccessToken.create!(application: app, resource_owner_id: user.id, scopes: 'mcp', expires_in: nil).update_columns(token: 'm2-token')
      ConnectedClient.create!(user:, oauth_application: app, mcp_tools: McpParity::NAMES + NAMES)
      File.write(File.join(dir, 'steps.json'), JSON.pretty_generate(steps))
      File.write(File.join(dir, 'market.json'), JSON.pretty_generate(sc['script']))
      ActiveRecord::Base.connection_pool.disconnect!
    end
    FileUtils.remove_entry(template)
  end
  def setup(dir)
    return unless File.exist?(File.join(dir, 'market.json'))
    Rails.configuration.dry_run = false
    Rails.cache = ActiveSupport::Cache::MemoryStore.new
    Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(ScriptedMarket::Adapter) unless Faraday::Adapter.lookup_middleware(:net_http_persistent).ancestors.include?(ScriptedMarket::Adapter)
    ScriptedMarket.http = JSON.parse(File.read(File.join(dir, 'market.json')))
    ScriptedMarket.requests = []
    ScriptedMarket.gaps = []
  end
end
