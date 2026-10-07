# D5b-2b-1: the populated first-sync page, recorded from the unchanged Rails controller.
D5_TRACKER_LIBRARY = true
require Rails.root.join('script/rust/pages_tracker')

before = Pages::BEFORE.merge(
  'first_unconfigured' => lambda {
    %w[market_data_provider market_data_url market_data_token coingecko_api_key].each { |key| AppConfig.set(key, nil) }
    Rails.cache.clear
  },
  'first_raw_usd' => -> { User.connection.execute("UPDATE users SET display_currency='usd'") },
  'first_foreign_exchange' => -> { Exchanges::Kraken.create!(name: 'Kraken', maker_fee: 0, taker_fee: 0) }
)
before['first_balance'] = lambda do
  asset = Asset.create!(external_id: 'd5-first-usd', symbol: 'USD', name: 'US Dollar', category: 'Currency', color: '#112233')
  AccountBalance.create!(user: User.first, exchange: Exchange.first, asset: asset, free: 10_000, locked: 0,
                         usd_price: 1, usd_value: 10_000, priced_at: Time.current, synced_at: Time.current)
end
before['first_transaction'] = lambda do
  AccountTransaction.create!(user: User.first, exchange: Exchange.first, entry_type: :deposit, base_currency: 'USD',
                             base_amount: 100, transacted_at: Time.current, tx_id: 'first-sync-record')
end
before['first_snapshot'] = -> { PortfolioSnapshot.create!(user: User.first, date: Date.current, value_usd: 100, held_value_usd: 0, held_cost_usd: 0) }
before['first_venue_snapshot'] = lambda do
  PortfolioVenueSnapshot.create!(user: User.first, exchange: Exchange.first, date: Date.current, value_usd: 100,
                                 held_value_usd: 0, held_cost_usd: 0)
end
before['first_balance_mark'] = -> { ApiKey.update_all(balances_synced_at: Time.current) }
before['first_ledger_mark'] = -> { ApiKey.update_all(last_synced_at: Time.current) }
before['first_failed'] = -> { ApiKey.update_all(last_sync_error: 'synthetic sync failure') }
before['first_other_venue'] = -> { Exchange.update_all(type: 'Exchanges::Ibkr') }
before['first_two_venues'] = lambda do
  exchange = Exchanges::Kraken.create!(name: 'Kraken', maker_fee: 0, taker_fee: 0)
  ApiKey.create!(user: User.first, exchange: exchange, key: 'synthetic-d5-paper-key', secret: 'synthetic-d5-paper-secret', status: :correct)
end
%w[EUR GBP CHF PLN].each do |currency|
  before["first_currency_#{currency}"] = lambda do
    User.first.update!(display_currency: currency)
    Rails.cache.write("exchange_rate_USD_to_#{currency}", Result::Success.new(0.8), expires_in: 12.hours)
  end
end
before['first_bad_settings'] = -> { User.first.update_column(:tracker_settings, []) }
before['first_fx_backoff'] = lambda do
  User.first.update!(display_currency: 'EUR')
  Rails.cache.write(Denomination.backoff_key('EUR'), true, expires_in: 5.minutes)
end
# Rust scheduler state is plain text, unlike AppConfig's encrypted attribute writer.
job_state = lambda do |job, key_id, value|
  c = AppConfig.connection
  c.execute('INSERT INTO app_configs(key,value,created_at,updated_at) VALUES(' \
            "#{c.quote("rust_job.#{job}:#{key_id}")},#{c.quote(value)},#{c.quote(Time.current)},#{c.quote(Time.current)})")
end
# Both API-key scoped jobs are enumerated in rust/src/sync/jobs.rs and jobs/resolve.rs.
%w[ledger_sync balance_sync].each do |job|
  {
    'success' => { last_run_at: '2026-10-07T00:00:00Z', last_success_at: '2026-10-07T00:00:00Z' }.to_json,
    'failed' => { last_error_at: '2026-10-07T00:00:00Z', last_error: 'synthetic failure' }.to_json,
    'incomplete' => { incomplete_since: '2026-10-07T00:00:00Z' }.to_json,
    'empty' => '{}', 'malformed' => 'invalid JSON', 'null' => nil
  }.each do |state, value|
    before["first_job_#{job}_#{state}"] = lambda do
      Exchanges::Kraken.create!(name: 'Kraken', maker_fee: 0, taker_fee: 0)
      job_state.call(job, ApiKey.first.id, value)
    end
  end
  before["first_job_#{job}_foreign"] = lambda do
    foreign = User.create!(Pages.owner('email' => 'foreign@example.com').merge('password' => Pages::PASSWORD))
    key = ApiKey.first.dup
    key.user = foreign
    key.save!
    job_state.call(job, key.id, { last_success_at: '2026-10-07T00:00:00Z' }.to_json)
  end
end
Pages.send(:remove_const, :BEFORE)
Pages.const_set(:BEFORE, before.freeze)

module D5FirstSyncGrid
  def scenarios
    cases = {}
    add = lambda do |name, path = '/tracker', headers = {}, hook = nil, attrs = {}, fixture = 'alpaca'|
      step = Pages.get(path, headers).merge('action_snapshot' => true, 'expect' => 200)
      step['before'] = hook if hook
      cases["first_#{name}"] = { 'user' => Pages.owner(attrs), 'd5_fixture' => fixture,
                                 'steps' => Pages.signed_in(step) }
    end
    I18n.available_locales.each do |locale|
      %w[full frame].each do |layout|
        headers = layout == 'frame' ? { 'Turbo-Frame' => 'modal' } : {}
        add.call("#{locale}_#{layout}", "/#{locale}/tracker", headers)
        add.call("hidden_#{locale}_#{layout}", "/#{locale}/tracker", headers, nil, 'hide_balances' => true)
      end
    end
    %w[EUR GBP CHF PLN].each { |currency| add.call("currency_#{currency}", '/tracker', {}, "first_currency_#{currency}") }
    add.call('fx_backoff', '/tracker', {}, 'first_fx_backoff')
    add.call('cash', '/tracker', {}, nil, 'tracker_settings' => { 'show_cash' => true })
    add.call('cash_string', '/tracker', {}, nil, 'tracker_settings' => { 'show_cash' => 'false' })
    add.call('read_only', '/tracker', {}, nil, {}, 'alpaca_read_only')
    add.call('unconfigured', '/tracker', {}, 'first_unconfigured')
    add.call('raw_usd', '/tracker', {}, 'first_raw_usd')
    add.call('scope', '/tracker?exchange_id=1')
    add.call('scope_suffix', '/tracker?exchange_id=1junk')
    add.call('scope_foreign', '/tracker?exchange_id=2', {}, 'first_foreign_exchange')
    add.call('blank_scope', '/tracker?exchange_id=')
    add.call('page_overflow', '/tracker?page=9223372036854775808')
    add.call('all_zero', '/tracker?all=0')
    add.call('ignored_filters', '/tracker?type=buy&status=open&show_cash=1')
    add.call('last_blank', '/tracker?from=2024-01-01&from=&to=')
    add.call('dates', '/tracker?from=2024-01-01&to=2026-09-10')
    add.call('dates_reversed', '/tracker?from=2026-09-11&to=2024-01-01')
    add.call('dates_last', '/tracker?from=invalid&from=2024-01-01&to=&to=2026-09-10')
    %w[from to].each do |key|
      add.call("deferred_#{key}_calendar_gap", "/tracker?#{key}=1582-10-10")
      cases["first_deferred_#{key}_calendar_gap"]['steps'].last.merge!('expect' => 500, 'rails_exception_status' => 500)
      add.call("deferred_#{key}_julian_leap", "/tracker?#{key}=1500-02-29")
      add.call("#{key}_gregorian_cutoff", "/tracker?#{key}=1583-01-01")
    end
    add.call('bad_iso', '/tracker?from=2024-02-30')
    cases['first_bad_iso']['steps'].last.merge!('expect' => 500, 'rails_exception_status' => 500)
    add.call('bad_settings', '/tracker', {}, 'first_bad_settings')
    cases['first_bad_settings']['steps'].last.merge!('expect' => 500, 'rails_exception_status' => 500)
    add.call('bad_id', '/tracker?exchange_id=9223372036854775808')
    cases['first_bad_id']['steps'].last.merge!('expect' => 404, 'rails_exception_status' => 404)
    add.call('escaped_scope', '/tracker?exchange_id=1%22%3E%3Cscript%3E')
    %w[balance transaction snapshot venue_snapshot balance_mark ledger_mark failed other_venue two_venues].each do |name|
      add.call("deferred_#{name}", '/tracker', {}, "first_#{name}")
    end
    %w[ledger_sync balance_sync].each do |job|
      %w[success failed incomplete empty malformed null].each do |state|
        add.call("deferred_job_#{job}_#{state}", '/tracker?exchange_id=2&from=2024-01-01&to=2024-01-02',
                 {}, "first_job_#{job}_#{state}")
      end
      add.call("job_#{job}_foreign", '/tracker', {}, "first_job_#{job}_foreign")
    end
    add.call('deferred_withdrawal', '/tracker', {}, nil, {}, 'alpaca_withdrawal')
    add.call('deferred_incorrect', '/tracker', {}, nil, {}, 'alpaca_incorrect')
    add.call('deferred_pending', '/tracker', {}, nil, 'tracker_settings' => { 'pending_report' => { 'country' => 'US', 'year' => 2024 } })
    add.call('deferred_date_grammar', '/tracker?from=1%20January%202024')
    add.call('deferred_structured', '/tracker?from[day]=2024-01-01')
    cases['first_deferred_structured']['steps'].last.merge!('expect' => 500, 'rails_exception_status' => 500)
    cases
  end
end
Pages.singleton_class.prepend(D5FirstSyncGrid)
command, root = ARGV
raise 'usage: grid|record DIR' unless %w[grid record].include?(command) && root

Pages.public_send(command, root)
if command == 'record'
  Dir[File.join(root, '*/rails.json')].each do |path|
    result = JSON.parse(File.read(path))
    answer = result.fetch('responses').last
    scenario = JSON.parse(File.read(File.join(File.dirname(path), 'scenario.json')))
    raise 'unexpected status' unless answer.fetch('status') == scenario.fetch('steps').last.fetch('expect')
    raise 'network attempt' unless result.fetch('network').empty?
    raise 'primary rows changed on GET' unless answer.fetch('rows_before') == answer.fetch('rows_after') &&
                                               answer.fetch('other_rows_before') == answer.fetch('other_rows_after')
  end
  puts "Recorded #{Dir[File.join(root, '*/rails.json')].length} populated first-sync GETs"
end
