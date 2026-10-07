# D5b-2a: record the early tracker index pages using the unchanged Rails controller.
D5_TRACKER_LIBRARY = true
require Rails.root.join('script/rust/pages_tracker')

before = Pages::BEFORE.merge(
  'index_no_market' => lambda {
    %w[market_data_provider market_data_url market_data_token coingecko_api_key].each { |key| AppConfig.set(key, nil) }
    Rails.cache.clear
  },
  'index_raw_usd' => lambda {
    User.connection.execute("UPDATE users SET display_currency='usd'")
  }
)
before['index_crypto_key'] = lambda do
  before.fetch('index_no_market').call
  Exchange.update_all(type: 'Exchanges::Kraken')
end
before['index_foreign_activity'] = lambda do
  before.fetch('index_no_market').call
  foreign = User.create!(Pages.owner('email' => 'foreign@example.com').merge('password' => Pages::PASSWORD))
  AccountTransaction.update_all(user_id: foreign.id)
end
before['index_existing_exchange'] = lambda do
  Exchanges::Kraken.create!(name: 'Kraken', maker_fee: 0, taker_fee: 0)
end
before['index_unreadable_color'] = lambda do
  before.fetch('index_no_market').call
  asset = Asset.find_by!(symbol: 'BTC')
  asset.update_columns(color: '#abc')
  AccountBalance.create!(user: User.first, exchange: Exchange.first, asset: asset, free: 1, locked: 0,
                         usd_price: 10_000, usd_value: 10_000, priced_at: Time.current, synced_at: Time.current)
end
Pages.send(:remove_const, :BEFORE)
Pages.const_set(:BEFORE, before.freeze)

module D5IndexGrid
  def scenarios
    cases = {}
    add = lambda do |name, fixture, path, headers = {}, hook = nil, attrs = {}|
      step = Pages.get(path, headers).merge('action_snapshot' => true, 'expect' => 200)
      step['before'] = hook if hook
      cases["index_#{name}"] = { 'user' => Pages.owner(attrs), 'd5_fixture' => fixture,
                                 'steps' => Pages.signed_in(step) }
    end
    I18n.available_locales.each do |locale|
      %w[full frame].each do |layout|
        headers = layout == 'frame' ? { 'Turbo-Frame' => 'modal' } : {}
        add.call("empty_#{locale}_#{layout}", 'empty', "/#{locale}/tracker", headers)
        add.call("missing_#{locale}_#{layout}", 'ledger', "/#{locale}/tracker", headers, 'index_no_market')
      end
    end
    add.call('raw_usd', 'empty', '/tracker', {}, 'index_raw_usd')
    add.call('empty_unconfigured', 'empty', '/tracker', {}, 'index_no_market')
    add.call('cash', 'empty', '/tracker', {}, nil, 'tracker_settings' => { 'show_cash' => true })
    add.call('hidden', 'empty', '/tracker', {}, nil, 'hide_balances' => true)
    add.call('page_overflow', 'empty', '/tracker?page=9223372036854775808')
    add.call('all_zero', 'empty', '/tracker?all=0')
    add.call('missing_ignores_bad_filter', 'ledger', '/tracker?exchange_id=9223372036854775808&from=invalid', {}, 'index_no_market')
    add.call('broker_unconfigured', 'alpaca', '/tracker', {}, 'index_no_market')
    add.call('populated', 'ledger', '/tracker')
    add.call('filtered_empty', 'empty', '/tracker?from=2024-01-01&to=2024-02-01')
    add.call('exchange_overflow', 'empty', '/tracker?exchange_id=9223372036854775808')
    cases['index_exchange_overflow']['steps'].last.merge!('expect' => 404, 'rails_exception_status' => 404)
    add.call('bad_date', 'empty', '/tracker?from=invalid')
    add.call('crypto_key', 'alpaca', '/tracker', {}, 'index_crypto_key')
    add.call('foreign_activity', 'ledger', '/tracker', {}, 'index_foreign_activity')
    add.call('existing_exchange', 'empty', '/tracker?exchange_id=1junk', {}, 'index_existing_exchange')
    add.call('pending', 'empty', '/tracker', {}, nil, 'tracker_settings' => { 'pending_report' => { 'country' => 'US', 'year' => 2024 } })
    %w[from to].each do |key|
      add.call("#{key}_last_blank", 'empty', "/tracker?#{key}=2024-01-01&#{key}=")
      add.call("#{key}_last_date", 'empty', "/tracker?#{key}=&#{key}=2024-01-01")
    end
    add.call('unreadable_color_frame', 'ledger', '/tracker', { 'Turbo-Frame' => 'modal' }, 'index_unreadable_color')
    add.call('unreadable_color_full', 'ledger', '/tracker', {}, 'index_unreadable_color')
    cases['index_unreadable_color_full']['steps'].last.merge!('expect' => 500, 'rails_exception_status' => 500)
    cases['index_bad_date']['steps'].last.merge!('expect' => 500, 'rails_exception_status' => 500)
    cases
  end
end
Pages.singleton_class.prepend(D5IndexGrid)
command, root = ARGV
raise 'usage: grid|record DIR' unless %w[grid record].include?(command) && root

Pages.public_send(command, root)
if command == 'record'
  Dir[File.join(root, '*/rails.json')].each do |path|
    result = JSON.parse(File.read(path))
    scenario = JSON.parse(File.read(File.join(File.dirname(path), 'scenario.json')))
    step = scenario.fetch('steps').last
    answer = result.fetch('responses').last
    raise 'unexpected status' unless answer.fetch('status') == step.fetch('expect')
    raise 'network attempt' unless result.fetch('network').empty?
    raise 'primary rows changed on GET' unless answer.fetch('rows_before') == answer.fetch('rows_after') &&
                                               answer.fetch('other_rows_before') == answer.fetch('other_rows_after')
  end
  puts "Recorded #{Dir[File.join(root, '*/rails.json')].length} tracker GETs"
end
