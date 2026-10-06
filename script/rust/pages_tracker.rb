# Task 1 working oracle extension. Loaded outside the unmodified Rails application.
require 'json'
require 'fileutils'
OAUTH_PARITY = true
require Rails.root.join('script/rust/pages')

headers = Pages::HEADERS + ['content-disposition']
Pages.send(:remove_const, :HEADERS)
Pages.const_set(:HEADERS, headers.freeze)

# Inject legacy values after sign-in, bypassing both validations and assignment normalization.
D5_USER_SAVE_CASES = {
  'zone' => { 'time_zone' => 'Invalid/Zone' }, 'blank_zone' => { 'time_zone' => '' },
  'locale' => { 'locale' => 'invalid' }, 'blank_locale' => { 'locale' => '' },
  'currency' => { 'display_currency' => 'BTC' }, 'lower_currency' => { 'display_currency' => 'usd' },
  'jurisdiction' => { 'wash_sale_jurisdiction' => 'DE' }, 'email' => { 'email' => "\u3000" },
  'valid_context' => { 'name' => '', 'email' => 'legacy-format', 'locale' => nil, 'wash_sale_jurisdiction' => ' ' }
}.freeze
before = Pages::BEFORE.merge(D5_USER_SAVE_CASES.to_h do |name, attributes|
  ["d5_user_#{name}", lambda do
    connection = User.connection
    assignments = attributes.map { |column, value| "#{connection.quote_column_name(column)}=#{connection.quote(value)}" }
    connection.execute("UPDATE users SET #{assignments.join(',')} WHERE id=#{User.first.id}")
  end]
end)
Pages.send(:remove_const, :BEFORE)
Pages.const_set(:BEFORE, before.freeze)

module D5Grid
  TABLES = %w[account_transactions account_balances portfolio_snapshots portfolio_venue_snapshots fund_classifications historical_prices
              fx_rates].freeze
  def scenarios
    request = lambda do |method, path, form = {}, status = 200|
      { 'method' => method, 'path' => path, 'form' => form, 'csrf' => 'header',
        'headers' => Pages::TURBO.merge('Referer' => 'http://localhost:3000/tracker'),
        'action_snapshot' => true, 'expect' => status }
    end
    cases = {}
    add = lambda do |name, steps, fixture = 'empty', attrs = {}|
      cases["d5_#{name}"] = { 'user' => Pages.owner(attrs), 'd5_fixture' => fixture,
                              'steps' => Pages.signed_in(Pages.get('/tracker').merge('action_snapshot' => true, 'expect' => 200), *steps) }
    end
    get = lambda do |path, status = 200, headers = {}|
      Pages.get(path, headers).merge('action_snapshot' => true, 'expect' => status)
    end
    add.call('empty', [get.call('/tracker')])
    add.call('empty_frame', [get.call('/tracker', 200, 'Turbo-Frame' => 'modal')])
    add.call('import_new', [get.call('/tracker/import/new')])
    add.call('import_missing', [request.call('POST', '/tracker/import', {}, 422)])
    add.call('sync_empty', [request.call('POST', '/tracker/sync', {}, 204)])
    add.call('sync_alpaca', [request.call('POST', '/tracker/sync')], 'alpaca')
    add.call('sync_read_only', [request.call('POST', '/tracker/sync')], 'alpaca_read_only')
    add.call('sync_withdrawal', [request.call('POST', '/tracker/sync', {}, 204)], 'alpaca_withdrawal')
    add.call('sync_incorrect', [request.call('POST', '/tracker/sync', {}, 204)], 'alpaca_incorrect')
    add.call('export_empty', [get.call('/tracker/export')])
    add.call('export_rows', [get.call('/tracker/export')], 'ledger')
    add.call('export_filter', [get.call('/tracker/export?from=2024-03-01&to=2024-04-01')], 'ledger')
    add.call('export_safety', [get.call('/tracker/export')], 'export_safety')
    add.call('export_exchange', [get.call('/tracker/export?exchange_id=1')], 'export_safety')
    add.call('export_missing_exchange', [get.call('/tracker/export?exchange_id=99999', 404).merge('rails_exception_status' => 404)], 'ledger')
    add.call('export_bad_date', [get.call('/tracker/export?from=invalid', 500).merge('rails_exception_status' => 500)], 'ledger')
    add.call('export_modal', [get.call('/tracker/export_modal')], 'ledger')
    I18n.available_locales.each do |locale|
      add.call("modal_locale_#{locale}", [get.call("/#{locale}/tracker/export_modal")], 'ledger')
    end
    add.call('modal_settings', [get.call('/tracker/export_modal')], 'ledger',
             'tracker_settings' => { 'export_type' => 'transactions', 'country' => 'SK', 'year' => '2023',
                                     'stablecoin_as_fiat' => true, 'export_from' => '2024-03-01', 'export_to' => '2024-04-01' })
    add.call('modal_broker', [get.call('/tracker/export_modal')], 'broker_panel',
             'tracker_settings' => { 'country' => 'DE', 'report_scope' => 'broker' })
    add.call('modal_broker_empty', [get.call('/tracker/export_modal')], 'alpaca')
    add.call('modal_broker_foreign_country', [get.call('/tracker/export_modal')], 'broker_panel',
             'tracker_settings' => { 'country' => 'US', 'report_scope' => 'broker' })
    add.call('index_cold', [get.call('/tracker')], 'ledger')
    add.call('index_hidden', [get.call('/tracker')], 'ledger', 'hide_balances' => true)
    add.call('index_cash', [get.call('/tracker')], 'ledger', 'tracker_settings' => { 'show_cash' => true })
    add.call('index_filter', [get.call('/tracker?exchange_id=1&from=2024-03-01&to=2024-04-01')], 'ledger')
    add.call('index_all', [get.call('/tracker?all=1')], 'many')
    add.call('index_limit', [get.call('/tracker')], 'many')
    add.call('settings', [request.call('PATCH', '/tracker/save_export_settings',
                                       { 'country' => 'US', 'year' => '2024', 'export_type' => 'tax', 'report_scope' => 'crypto',
                                         'unpermitted' => 'ignored' })])
    add.call('settings_blank', [request.call('PATCH', '/tracker/save_export_settings', { 'country' => '', 'year' => ' ' })])
    D5_USER_SAVE_CASES.each_key do |name|
      add.call("settings_validation_#{name}", [request.call('PATCH', '/tracker/save_export_settings', { 'country' => 'US' })
        .merge('before' => "d5_user_#{name}")])
    end
    json_headers = Pages::TURBO.merge('Content-Type' => 'application/json', 'Referer' => 'http://localhost:3000/tracker')
    add.call('settings_query_json', [request.call('PATCH', '/tracker/save_export_settings?country=DE').merge(
      'json' => JSON.generate('country' => 'US', 'year' => 2024), 'headers' => json_headers
    )])
    add.call('fund_symbol_json', [request.call('PATCH', '/tracker/fund_classifications').merge(
      'json' => JSON.generate('classifications' => [{ 'symbol' => 123, 'kind' => 'share' }]), 'headers' => json_headers
    )])
    add.call('fund_share', [request.call('PATCH', '/tracker/fund_classifications',
                                         { 'classifications' => [{ 'symbol' => ' AaPl ', 'kind' => 'share', 'fund_category' => 'equity_fund' }] })])
    add.call('fund_valid', [request.call('PATCH', '/tracker/fund_classifications',
                                         { 'classifications' => [{ 'symbol' => 'ETF', 'kind' => 'fund', 'fund_category' => 'equity_fund' }] })])
    add.call('fund_invalid', [request.call('PATCH', '/tracker/fund_classifications',
                                           { 'classifications' => [{ 'symbol' => 'ETF', 'kind' => 'fund' }] }, 422)])
    add.call('fund_mixed', [request.call('PATCH', '/tracker/fund_classifications',
                                         { 'classifications' => [{ 'symbol' => 'ETF', 'kind' => 'fund' },
                                                                 { 'symbol' => 'AAPL', 'kind' => 'share' }] }, 422)])
    add.call('fund_bad_shape', [request.call('PATCH', '/tracker/fund_classifications',
                                             { 'classifications' => { '0' => { 'symbol' => 'AAPL', 'kind' => 'share' } } })])
    add.call('fund_unknown', [request.call('PATCH', '/tracker/fund_classifications',
                                           { 'classifications' => [{ 'symbol' => 'AAPL', 'kind' => 'unknown' }] })])
    %w[12.5 0 -1 abc].each do |price|
      status = %w[-1 abc].include?(price) ? 422 : 200
      add.call("price_#{price}", [request.call('PATCH', '/tracker/transactions/4/price', { 'price' => price }, status)], 'ledger')
    end
    add.call('price_clear', [request.call('PATCH', '/tracker/transactions/4/price', { 'price' => '12.5' }),
                             request.call('PATCH', '/tracker/transactions/4/price', { 'price' => '' })], 'ledger')
    add.call('price_venue', [request.call('PATCH', '/tracker/transactions/1/price', { 'price' => '12.5' }, 422)], 'ledger')
    add.call('link_unlink', [request.call('PATCH', '/tracker/transactions/5/toggle_transfer'),
                             request.call('PATCH', '/tracker/transactions/6/toggle_transfer')], 'ledger')
    add.call('link_missing', [request.call('PATCH', '/tracker/transactions/4/toggle_transfer')], 'ledger')
    I18n.available_locales.each do |locale|
      add.call("price_locale_#{locale}", [request.call('PATCH', "/#{locale}/tracker/transactions/4/price", { 'price' => '12.5' })], 'ledger')
    end
    add.call('price_hidden', [request.call('PATCH', '/tracker/transactions/4/price', { 'price' => '12.5' })], 'ledger', 'hide_balances' => true)
    add.call('price_missing', [request.call('PATCH', '/tracker/transactions/4/price', { 'price' => '' })], 'row_missing')
    add.call('price_small', [request.call('PATCH', '/tracker/transactions/4/price', { 'price' => '0.01' })], 'row_small')
    add.call('price_negative_adjustment', [request.call('PATCH', '/tracker/transactions/4/price', { 'price' => '12.5' })], 'row_adjustment')
    add.call('price_cash_eur', [request.call('PATCH', '/tracker/transactions/4/price', { 'price' => '' })], 'row_eur')
    add.call('price_flag', [request.call('PATCH', '/tracker/transactions/4/price', { 'price' => '' })], 'row_flag')
    add.call('price_group_clear', [request.call('PATCH', '/tracker/transactions/4/price', { 'price' => '' })], 'row_group')
    add.call('price_quote_clear', [request.call('PATCH', '/tracker/transactions/1/price', { 'price' => '' })], 'ledger')
    add.call('link_success', [request.call('PATCH', '/tracker/transactions/5/toggle_transfer')], 'ledger')
    add.call('tax_invalid', [get.call('/tracker/tax_report?country=ZZ&year=2024', 302)])
    add.call('tax_pending', [get.call('/tracker/tax_report?country=US&year=2024')], 'ledger')
    add.call('download_expired', [get.call('/tracker/download_tax_report?country=US&year=2024', 302)])
    %w[sync settings fund price link import].each do |kind|
      method, path, fields = {
        'sync' => ['POST', '/tracker/sync', {}], 'settings' => ['PATCH', '/tracker/save_export_settings', { 'country' => 'DE' }],
        'fund' => ['PATCH', '/tracker/fund_classifications', { 'classifications' => [{ 'symbol' => 'AAPL', 'kind' => 'share' }] }],
        'price' => ['PATCH', '/tracker/transactions/4/price', { 'price' => '12' }],
        'link' => ['PATCH', '/tracker/transactions/5/toggle_transfer', {}], 'import' => ['POST', '/tracker/import', {}]
      }.fetch(kind)
      step = request.call(method, path, fields, 302).merge('csrf' => 'none')
      add.call("csrf_#{kind}", [step], 'ledger')
    end
    [true, false, 123, 1.25, nil].each_with_index do |symbol, index|
      add.call("fund_cast_#{index}", [request.call('PATCH', '/tracker/fund_classifications').merge(
        'json' => JSON.generate('classifications' => [{ 'symbol' => symbol, 'kind' => 'share' }]), 'headers' => json_headers
      )])
    end
    ['application/json', 'text/html', '*/*'].each_with_index do |accept, index|
      { 'price' => ['4/price', { 'price' => '12.5' }], 'link' => ['5/toggle_transfer', {}] }.each do |kind, (route, form)|
        step = request.call('PATCH', "/tracker/transactions/#{route}", form)
        step['headers']['Accept'] = accept
        add.call("#{kind}_accept_#{index}", [step], 'ledger')
      end
    end
    add.call('link_sql_reverse', [request.call('PATCH', '/tracker/transactions/6/toggle_transfer')], 'sql_precision')
    add.call('link_sql_precision', [request.call('PATCH', '/tracker/transactions/5/toggle_transfer')], 'sql_precision')
    add.call('price_blank_currency',
             [request.call('PATCH', '/tracker/transactions/4/price', { 'price' => '12' }, 422).merge('rails_exception_status' => 422)],
             'blank_currency')
    overflow = request.call('PATCH', '/tracker/transactions/9223372036854775808/price', { 'price' => '12' }, 404)
    add.call('price_overflow', [overflow.merge('rails_exception_status' => 404)], 'maximum_id')
    cases.select { |name, _| name.match?(/\Ad5_(?:settings|fund_|sync_|price_|link_|modal_|export_modal\z)/) }
  end

  def action_fixture(scenario)
    super
    return unless scenario['d5_fixture']

    AppConfig.market_data_provider = 'deltabadger'
    AppConfig.market_data_url = 'http://127.0.0.1:1'
    AppConfig.market_data_token = 'synthetic-d5-market-token'
    FxRate.create!(currency: 'USD', date: Date.new(2026, 9, 10), rate: '1.1')
    return if scenario['d5_fixture'] == 'empty'

    user = User.first
    if scenario['d5_fixture'].start_with?('alpaca')
      exchange = Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: 0, taker_fee: 0)
      kind = case scenario['d5_fixture']
             when 'alpaca_read_only' then :read_only
             when 'alpaca_withdrawal' then :withdrawal
             else :trading
             end
      status = scenario['d5_fixture'] == 'alpaca_incorrect' ? :incorrect : :correct
      ApiKey.create!(user: user, exchange: exchange, key: 'synthetic-d5-paper-key', secret: 'synthetic-d5-paper-secret',
                     key_type: kind, status: status)
      return
    end

    exchange = Exchanges::Binance.create!(name: 'Binance', maker_fee: '0.1', taker_fee: '0.1')
    Asset.create!(external_id: 'bitcoin', symbol: 'BTC', name: 'Bitcoin', category: 'Cryptocurrency', color: '#F7931A')
    entries = [[:buy, 'BTC', '10', '10000'], [:return_of_capital, 'BTC', '10', '15000'], [:sell, 'BTC', '10', '100000'],
               [:airdrop, 'BTC', '1', nil], [:withdrawal, 'USD', '100', nil], [:deposit, 'USD', '99', nil]]
    entries.each_with_index do |(kind, symbol, amount, quote), index|
      AccountTransaction.create!(user: user, exchange: exchange, entry_type: kind, base_currency: symbol,
                                 base_amount: amount, quote_currency: ('USD' if quote), quote_amount: quote,
                                 transacted_at: (index == 5 ? Time.utc(2024, 5, 2) : Time.utc(2024, index + 1, 1)),
                                 tx_id: "d5-#{index}", raw_data: {})
    end
    HistoricalPrice.create!(asset: 'BTC', currency: 'USD', date: Date.new(2024, 4, 1), price: '10000')
    case scenario['d5_fixture']
    when 'sql_precision'
      AccountTransaction.where(id: 5).update_all(base_amount: 1)
      AccountTransaction.connection.execute('UPDATE account_transactions SET base_amount=1.0000000000000002 WHERE id=6')
    when 'blank_currency'
      AccountTransaction.where(id: 4).update_all(base_currency: '')
    when 'maximum_id'
      AccountTransaction.where(id: 4).update_all(id: 9_223_372_036_854_775_807)
    when 'row_missing'
      HistoricalPrice.delete_all
    when 'row_small'
      AccountTransaction.find(4).update!(base_amount: '0.0000001', fee_amount: '-0.00000001', fee_currency: 'BTC')
    when 'row_adjustment'
      AccountTransaction.find(4).update!(entry_type: :adjustment, base_amount: '-2', fee_amount: '-0.25', fee_currency: 'USD')
    when 'row_eur', 'row_flag'
      AccountTransaction.find(4).update!(base_currency: 'EUR')
      FxRate.create!(currency: 'USD', date: Date.new(2024, 4, 1), rate: '1.1')
      Asset.create!(external_id: 'euro', symbol: 'EUR', name: 'Euro', category: 'Fiat') if scenario['d5_fixture'] == 'row_flag'
    when 'row_group'
      AccountTransaction.find(4).update!(entry_type: :buy, group_id: 'g')
      AccountTransaction.create!(user: user, exchange: exchange, entry_type: :sell, base_currency: 'USD', base_amount: '12',
                                 transacted_at: Time.utc(2024, 4, 1), group_id: 'g')
    end
    if scenario['d5_fixture'] == 'broker_panel'
      broker = Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: 0, taker_fee: 0)
      [['AAA', 'stock', :buy, 2024], ['ETF', 'etf', :buy, 2024], ['OLD', 'etf', :buy, 2017],
       ['UNK', nil, :buy, 2024], ['REF', 'stock', :unsupported_activity, 2024],
       ['PERSIST', 'etf', :buy, 2024], ['FUTURE', 'stock', :buy, 2026]].each do |symbol, instrument, kind, year|
        Asset.create!(external_id: "stock-#{symbol}", symbol: symbol, name: symbol, category: 'Stock', instrument_type: instrument)
        AccountTransaction.create!(user: user, exchange: broker, entry_type: kind, base_currency: symbol, base_amount: '1',
                                   transacted_at: Time.utc(year, 1, 1), raw_data: {})
      end
      foreign = User.create!(name: 'Foreign', email: 'broker-foreign@example.test', password: 'Password123!', password_confirmation: 'Password123!')
      AccountTransaction.create!(user: foreign, exchange: broker, entry_type: :buy, base_currency: 'FOREIGN', base_amount: '1',
                                 transacted_at: Time.utc(2024, 1, 1), raw_data: {})
      FundClassification.create!(user: user, symbol: 'REF', kind: :share)
      FundClassification.create!(user: user, symbol: 'PERSIST', kind: :fund, fund_category: :equity_fund)
    end
    if scenario['d5_fixture'] == 'export_safety'
      AccountTransaction.order(:id).zip(['=1+1', ' +1', "\t@SUM(1,2)", "line\n\"quoted\"", '', nil]).each do |row, description|
        row.update!(description: description)
      end
      AccountTransaction.first.update!(fee_currency: 'USD', fee_amount: '-0.25')
      foreign = User.create!(name: 'Foreign', email: 'foreign@example.test', password: 'Password123!', password_confirmation: 'Password123!')
      AccountTransaction.create!(user: foreign, exchange: exchange, entry_type: :airdrop, base_currency: 'FOREIGN',
                                 base_amount: '999', transacted_at: Time.utc(2024, 4, 1), raw_data: {})
    end
    return unless scenario['d5_fixture'] == 'many'

    201.times do |i|
      AccountTransaction.create!(user: user, exchange: exchange, entry_type: :buy, base_currency: 'BTC',
                                 base_amount: '1', quote_currency: 'USD', quote_amount: '10000', transacted_at: Time.utc(2024, 7, 1) + i,
                                 tx_id: "many-#{i}", raw_data: {})
    end
  end

  # Oj's Rails defaults serialize 1.0000000000000002 as 1. Preserve SQLite
  # REAL bit patterns in oracle snapshots; this does not change app rendering.
  def lossless_rows(tables)
    tables.transform_values do |rows|
      rows.map do |row|
        row.transform_values { |value| value.is_a?(Float) ? { '__sqlite_real__' => [value].pack('G').unpack1('H*') } : value }
      end
    end
  end

  def action_other_rows
    lossless_rows(super)
  end

  def action_rows
    lossless_rows(super.merge(TABLES.to_h do |table|
      [table, ActiveRecord::Base.connection.select_all("SELECT * FROM #{table} ORDER BY id").to_a]
    end))
  end
end
Pages.singleton_class.prepend(D5Grid)
Rails.cache = ActiveSupport::Cache::MemoryStore.new
command, root = ARGV
raise 'usage: grid|record DIR' unless %w[grid record].include?(command) && root

Pages.public_send(command, root)
if command == 'record'
  Dir[File.join(root, '*/rails.json')].each do |path|
    scenario = JSON.parse(File.read(File.join(File.dirname(path), 'scenario.json')))
    result = JSON.parse(File.read(path))
    raise "network attempt in #{File.basename(File.dirname(path))}" unless result.fetch('network').empty?

    scenario.fetch('steps').zip(result.fetch('responses')).each do |step, answer|
      next unless step.key?('expect')
      raise "unexpected status for #{step['method']} #{step['path']}: #{answer['status']}" unless step['expect'] == answer['status']
    end
  end
  puts "recorded and checked #{Dir[File.join(root, '*/rails.json')].size} scenarios"
end
