# Cold-ledger holdings, from the unchanged Rails controller and views.
D5_TRACKER_LIBRARY = true
require Rails.root.join('script/rust/pages_tracker')
require 'digest'
before = Pages::BEFORE.dup
%w[normal cash stale unpriced missing_price folded single_tail hidden_tail zero_value].each do |kind|
  before["holdings_#{kind}"] = lambda do
    Rails.cache.clear
    symbols = kind == 'cash' ? ['USD'] : %w[AAPL VTI BTC USDC USD MSFT NVDA AMZN]
    values = [500, 300, 100, 60, 40, 10, 2, 1]
    symbols.each_with_index do |symbol, i|
      category = { 'USD' => 'Currency', 'USDC' => 'Cryptocurrency', 'BTC' => 'Cryptocurrency', 'VTI' => 'ETF' }.fetch(symbol, 'Stock')
      asset = Asset.create!(external_id: "holdings-#{symbol}", symbol: symbol, name: "#{symbol} <&> holding", category: category, color: '#112233')
      value = values[i]
      value = nil if kind == 'unpriced' && i.zero?
      value = 0 if kind == 'zero_value' && i.zero?
      price_time = Time.current - (kind == 'stale' ? 301 : 0)
      price_time = nil if kind == 'missing_price' && i.zero?
      AccountBalance.create!(user: User.first, exchange: Exchange.first, asset: asset, free: i + 1, locked: 0,
                             usd_price: value && (value.to_d / (i + 1)), usd_value: value,
                             priced_at: price_time,
                             synced_at: Time.current)
    end
    ApiKey.update_all(balances_synced_at: Time.current)
  end
end
%w[transaction snapshot venue_snapshot warm pending].each do |kind|
  before["holdings_deferred_#{kind}"] = lambda do
    before.fetch('holdings_normal').call
    case kind
    when 'transaction'
      AccountTransaction.create!(user: User.first, exchange: Exchange.first, entry_type: :deposit, base_currency: 'USD',
                                 base_amount: 1, transacted_at: Time.current)
    when 'snapshot'
      PortfolioSnapshot.create!(user: User.first, date: Date.current, value_usd: 1)
    when 'venue_snapshot'
      PortfolioVenueSnapshot.create!(user: User.first, exchange: Exchange.first, date: Date.current, value_usd: 1)
    when 'warm'
      Tracker::Ledger.compute!(User.first)
      c = AppConfig.connection
      c.execute('INSERT INTO app_configs(key,value,created_at,updated_at) VALUES(' \
                "'rust_tracker_ledger.#{User.first.id}','{}',#{c.quote(Time.current)},#{c.quote(Time.current)})")
    when 'pending'
      User.first.update!(tracker_settings: { 'pending_report' => { 'country' => 'US', 'year' => 2024 } })
    end
  end
end
%w[EUR GBP CHF PLN].each do |currency|
  before["holdings_currency_#{currency}"] = lambda do
    before.fetch('holdings_normal').call
    User.first.update!(display_currency: currency)
    rate = { 'EUR' => 0.8, 'GBP' => 0.5, 'CHF' => 0.9, 'PLN' => 4.0 }.fetch(currency)
    Rails.cache.write("exchange_rate_USD_to_#{currency}", Result::Success.new(rate), expires_in: 12.hours)
  end
end
before['holdings_fx_unavailable'] = lambda do
  before.fetch('holdings_normal').call
  User.first.update!(display_currency: 'EUR')
  Rails.cache.write(Denomination.backoff_key('EUR'), true, expires_in: 5.minutes)
end
before['holdings_raw_usd'] = lambda do
  before.fetch('holdings_normal').call
  User.connection.execute("UPDATE users SET display_currency='usd'")
end
[59, 60, 3600, 86_400, 604_800, 2_419_200, 31_536_000, -60].each do |seconds|
  before["holdings_age_#{seconds}"] = lambda do
    before.fetch('holdings_normal').call
    AccountBalance.update_all(priced_at: Time.current - seconds, synced_at: Time.current - seconds)
    ApiKey.update_all(balances_synced_at: Time.current - seconds)
  end
end
before['holdings_foreign_scope'] = lambda do
  before.fetch('holdings_normal').call
  Exchanges::Kraken.create!(name: 'Kraken', maker_fee: 0, taker_fee: 0)
end
before['holdings_single_tail'] = lambda do
  before.fetch('holdings_normal').call
  AccountBalance.where.not(asset: Asset.where(symbol: %w[AAPL AMZN])).delete_all
end
before['holdings_zero_quantity'] = lambda do
  before.fetch('holdings_normal').call
  AccountBalance.where(asset: Asset.find_by(symbol: 'AAPL')).update_all(free: 0, locked: 0)
end
%w[name symbol labels].each do |field|
  before["holdings_null_#{field}"] = lambda do
    before.fetch('holdings_normal').call
    attrs = field == 'labels' ? { name: nil, symbol: nil } : { field => nil }
    Asset.find_by(symbol: 'AAPL').update_columns(attrs)
  end
end
Pages.send(:remove_const, :BEFORE)
Pages.const_set(:BEFORE, before.freeze)
module D5HoldingsGrid
  def scenarios
    cases = {}
    add = lambda do |name, path, headers = {}, hook = 'normal', attrs = {}|
      step = Pages.get(path, headers).merge('action_snapshot' => true, 'expect' => 200, 'before' => "holdings_#{hook}")
      cases["holdings_#{name}"] = { 'user' => Pages.owner(attrs), 'd5_fixture' => 'alpaca', 'steps' => Pages.signed_in(step) }
    end
    I18n.available_locales.each do |locale|
      %w[full frame].each do |layout|
        headers = layout == 'frame' ? { 'Turbo-Frame' => 'modal' } : {}
        add.call("#{locale}_#{layout}", "/#{locale}/tracker", headers)
        add.call("hidden_#{locale}_#{layout}", "/#{locale}/tracker", headers, 'normal', 'hide_balances' => true)
        add.call("cash_#{locale}_#{layout}", "/#{locale}/tracker", headers, 'normal', 'tracker_settings' => { 'show_cash' => true })
      end
    end
    %w[cash stale unpriced missing_price zero_value].each { |kind| add.call(kind, '/tracker', {}, kind) }
    %w[transaction snapshot venue_snapshot warm pending].each { |kind| add.call("deferred_#{kind}", '/tracker', {}, "deferred_#{kind}") }
    add.call('scope', '/tracker?exchange_id=1')
    add.call('dates', '/tracker?from=2024-01-01&to=2024-12-31')
    add.call('last_blank', '/tracker?from=invalid&from=')
    add.call('page_overflow', '/tracker?page=9223372036854775808&all=1')
    %w[EUR GBP CHF PLN].each { |currency| add.call("currency_#{currency}", '/tracker', {}, "currency_#{currency}") }
    %w[fx_unavailable raw_usd single_tail zero_quantity].each { |kind| add.call(kind, '/tracker', {}, kind) }
    [59, 60, 3600, 86_400, 604_800, 2_419_200, 31_536_000, -60].each { |n| add.call("age_#{n}", '/tracker', {}, "age_#{n}") }
    add.call('foreign_scope', '/tracker?exchange_id=2', {}, 'foreign_scope')
    add.call('overflow_scope', '/tracker?exchange_id=9223372036854775808')
    cases['holdings_overflow_scope']['steps'].last.merge!('expect' => 404, 'rails_exception_status' => 404)
    add.call('stale_hidden', '/tracker', {}, 'stale', 'hide_balances' => true)
    add.call('scope_suffix', '/tracker?exchange_id=1junk')
    add.call('cash_string', '/tracker', {}, 'normal', 'tracker_settings' => { 'show_cash' => 'false' })
    add.call('dates_reverse', '/tracker?from=2026-01-01&to=2024-01-01')
    add.call('dates_bad', '/tracker?from=2024-02-30')
    cases['holdings_dates_bad']['steps'].last.merge!('expect' => 500, 'rails_exception_status' => 500)
    %w[name symbol labels].each { |field| add.call("null_#{field}", '/tracker', {}, "null_#{field}") }
    cases
  end
end
Pages.singleton_class.prepend(D5HoldingsGrid)
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
    unless answer.fetch('rows_before') == answer.fetch('rows_after') && answer.fetch('other_rows_before') == answer.fetch('other_rows_after')
      raise 'GET changed rows'
    end
  end
end

if command == 'record'
  sources = %w[
    app/controllers/tracker_controller.rb
    app/models/tracker/figures.rb
    app/models/account_balance.rb
    app/models/user.rb
    app/models/denomination.rb
    app/models/utilities/currency.rb
    app/models/portfolio_snapshot.rb
    app/models/api_key.rb
    app/helpers/tracker_helper.rb
    app/helpers/application_helper.rb
    app/views/assets/_logo.html.erb
    app/views/tracker/index.html.erb
    app/views/tracker/_holdings.html.erb
    app/views/tracker/_holding_row.html.erb
    app/views/tracker/_tiles.html.erb
    app/views/tracker/_record.html.erb
    app/views/tracker/_exchange_filters.html.erb
    app/views/tracker/_chart.html.erb
    app/views/bots/_metrics_item.html.erb
    Gemfile.lock
  ]
  hashes = sources.to_h { |path| [path, Digest::SHA256.file(Rails.root.join(path)).hexdigest] }
  File.write(File.join(root, 'sources.json'), "#{JSON.pretty_generate(hashes)}\n")
end
