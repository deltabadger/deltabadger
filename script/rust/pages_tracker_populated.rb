# The populated tracker index (venue filters, tiles, holdings, sync warnings, pending report), from the unchanged Rails
# controller and views, on an account with history on two venues and a pending third key.
#   bin/rails runner script/rust/pages_tracker_populated.rb grid|record DIR
D5_TRACKER_LIBRARY = true
require Rails.root.join('script/rust/pages_tracker')
require 'digest'

module PopulatedFixture
  module_function

  def asset(symbol, name, category, color)
    Asset.find_by(symbol: symbol, category: category) ||
      Asset.create!(external_id: "populated-#{symbol.downcase}", symbol: symbol, name: name, category: category, color: color)
  end

  def row(user, exchange, kind, symbol, amount, at, quote: nil, fee: nil)
    AccountTransaction.create!(user: user, exchange: exchange, entry_type: kind, base_currency: symbol, base_amount: amount,
                               quote_currency: ('USD' if quote), quote_amount: quote, fee_currency: ('USD' if fee), fee_amount: fee,
                               transacted_at: Time.iso8601(at), tx_id: "populated-#{AccountTransaction.count}", raw_data: {})
  end

  def balance(user, exchange, asset, free, price, priced_at: Time.current)
    AccountBalance.create!(user: user, exchange: exchange, asset: asset, free: free, locked: 0, usd_price: price,
                           usd_value: price && (free.to_d * price.to_d).round(8), priced_at: price && priced_at, synced_at: Time.current)
  end

  # Alpaca (the fixture's key) and Binance with history, a pending IBKR key, and two venues the + can still add.
  def build(kind)
    Rails.cache.clear
    user = User.first
    alpaca = Exchanges::Alpaca.first
    binance = Exchanges::Binance.create!(name: 'Binance', maker_fee: '0.1', taker_fee: '0.1')
    ibkr = Exchanges::Ibkr.create!(name: 'Interactive Brokers', maker_fee: 0, taker_fee: 0)
    Exchanges::Kraken.create!(name: 'Kraken', maker_fee: 0, taker_fee: 0)
    Exchanges::Bitmart.create!(name: 'BitMart', maker_fee: 0, taker_fee: 0)
    binance_key = ApiKey.create!(user: user, exchange: binance, key: 'synthetic-d5-binance-key', secret: 'synthetic-d5-binance-secret',
                                 key_type: :trading, status: :correct)
    ApiKey.create!(user: user, exchange: ibkr, key: 'synthetic-d5-ibkr-key', secret: 'synthetic-d5-ibkr-secret',
                   key_type: :trading, status: :pending_validation)
    usd = asset('USD', 'US Dollar', 'Currency', '#85BB65')
    aapl = asset('AAPL', 'Apple <&> Inc.', 'Stock', '#A2AAAD')
    msft = asset('MSFT', 'Microsoft', 'Stock', '#00A4EF')
    btc = asset('BTC', 'Bitcoin', 'Cryptocurrency', '#F7931A')
    row(user, alpaca, :deposit, 'USD', '1000', '2026-09-01T14:00:00Z')
    row(user, alpaca, :buy, 'AAPL', '2', '2026-09-02T14:30:00Z', quote: '400')
    row(user, alpaca, :buy, 'MSFT', '1', '2026-09-03T14:30:00Z', quote: '100')
    row(user, alpaca, :sell, 'MSFT', '1', '2026-09-04T14:30:00Z', quote: '120')
    row(user, binance, :deposit, 'USD', '500', '2026-09-01T10:00:00Z')
    row(user, binance, :buy, 'BTC', '0.01', '2026-09-02T10:00:00Z', quote: '300', fee: '0.3')
    balance(user, alpaca, usd, '620', '1')
    balance(user, alpaca, aapl, '2', '250')
    balance(user, binance, usd, '199.7', '1')
    balance(user, binance, btc, '0.01', '60000')
    ApiKey.where(status: :correct).update_all(balances_synced_at: Time.current)
    case kind
    when 'many'
      %w[NVDA AMZN GOOG META TSLA].each_with_index do |symbol, i|
        stock = asset(symbol, symbol, 'Stock', '#112233')
        row(user, alpaca, :buy, symbol, '1', "2026-09-05T14:3#{i}:00Z", quote: (10 + i).to_s)
        balance(user, alpaca, stock, '1', (12 + i).to_s)
      end
    when 'notes'
      eth = asset('ETH', 'Ether', 'Cryptocurrency', '#627EEA')
      # Bought after the sync (held at cost until the next one), and a coin no venue reports any more.
      row(user, binance, :buy, 'ETH', '0.1', (Time.current + 1).utc.iso8601(6), quote: '100')
      row(user, binance, :buy, 'SOL', '1', '2026-09-03T10:00:00Z', quote: '50')
      asset('SOL', 'Solana', 'Cryptocurrency', '#9945FF')
      eth
    when 'stale'
      AccountBalance.where(asset: btc).update_all(priced_at: Time.current - 301)
    when 'unpriced'
      AccountBalance.where(asset: btc).update_all(usd_price: nil, usd_value: nil, priced_at: nil)
    when 'failed'
      binance_key.update_columns(last_sync_error: 'Binance answered 500')
    when 'dead'
      binance_key.update_columns(status: ApiKey.statuses[:incorrect], last_sync_error: 'Invalid API-key, IP, or permissions for action.')
    when 'history', 'long'
      # A swept history: the whole and each venue, with the positions' own readings (show cash off), and one
      # day the sweep could not value without the cash (`held_cost_usd` nil).
      first = kind == 'long' ? Date.new(2026, 6, 1) : Date.new(2026, 9, 1)
      (first..Date.new(2026, 9, 9)).each_with_index do |day, i|
        held = day == Date.new(2026, 9, 5) ? nil : (600 + i * 3.25).to_d
        PortfolioSnapshot.create!(user: user, date: day, value_usd: 1000 + i * 4.5, invested_usd: 900 + i, held_value_usd: held,
                                  held_cost_usd: held && (500 + i))
        [alpaca, binance].each_with_index do |venue, v|
          PortfolioVenueSnapshot.create!(user: user, exchange: venue, date: day, value_usd: 500 + i * (v + 1), invested_usd: 450 + v,
                                         held_value_usd: 300 + i, held_cost_usd: 250 + v)
        end
      end
    when 'rows'
      # More rows than the table shows, a transfer between the venues, and a price the user stated.
      HistoricalPrice.create!(asset: 'DOT', currency: 'USD', date: Date.new(2026, 9, 6), price: '4.25')
      asset('DOT', 'Polkadot', 'Cryptocurrency', '#E6007A')
      out = row(user, binance, :withdrawal, 'USD', '20', '2026-09-06T09:00:00Z')
      into = row(user, alpaca, :deposit, 'USD', '20', '2026-09-06T09:05:00Z')
      out.update_columns(linked_transaction_id: into.id)
      stated = row(user, binance, :airdrop, 'DOT', '2', '2026-09-06T12:00:00Z')
      stated.update_columns(manual_values: { 'price' => '4.5' })
      row(user, binance, :airdrop, 'DOT', '1', '2026-09-06T13:00:00Z')
      200.times { |i| row(user, alpaca, :buy, 'AAPL', '0.001', (Time.utc(2026, 9, 7, 10) + (i * 60)).iso8601, quote: '0.2') }
      # The round trip's own rows record their asset: the positions row draws it.
      AccountTransaction.where(base_currency: 'MSFT').update_all(base_asset_id: msft.id)
    when 'santiago', 'magadan'
      # A19 zones: one row inside the UTC day but outside the user's local day (the band only Rails' UTC cut takes), one
      # inside both, none in the reverse band. Tiny amounts: no note, no history-gap line.
      day = kind == 'santiago' ? '2026-09-06' : '2014-10-26'
      outside, inside = kind == 'santiago' ? %w[02:00 12:00] : %w[20:00 05:00]
      row(user, binance, :buy, 'BTC', '0.000001', "#{day}T#{outside}:00Z", quote: '0.06')
      row(user, binance, :buy, 'BTC', '0.000001', "#{day}T#{inside}:00Z", quote: '0.06')
    when 'lone'
      AccountTransaction.where(exchange: binance).delete_all
      AccountBalance.where(exchange: binance).delete_all
      ApiKey.where(exchange: [binance, ibkr]).delete_all
    end
    # Warm: the ledger the job caches. Cold: nothing cached yet (and for a key that stopped reading, whose venue the
    # Rust producer does not publish over).
    Tracker::Ledger.compute!(user) unless %w[cold dead].include?(kind)
  end
end

before = Pages::BEFORE.dup
%w[base many notes stale unpriced failed dead lone cold history long rows santiago magadan].each do |kind|
  before["populated_#{kind}"] = -> { PopulatedFixture.build(kind) }
end
{ 'EUR' => 0.8, 'GBP' => 0.5, 'CHF' => 0.9, 'PLN' => 4.0 }.each do |currency, rate|
  before["populated_currency_#{currency}"] = lambda do
    PopulatedFixture.build('base')
    User.first.update!(display_currency: currency)
    Rails.cache.write("exchange_rate_USD_to_#{currency}", Result::Success.new(rate), expires_in: 12.hours)
  end
end
# A19: a display currency with no rate held (EUR is populated_fx_unavailable), and a swept history with none.
%w[GBP CHF PLN].each do |currency|
  before["populated_fx_unavailable_#{currency}"] = lambda do
    PopulatedFixture.build('base')
    User.first.update!(display_currency: currency)
    Rails.cache.write(Denomination.backoff_key(currency), true, expires_in: 5.minutes)
  end
end
before['populated_fx_unavailable_history'] = lambda do
  PopulatedFixture.build('history')
  User.first.update!(display_currency: 'EUR')
  Rails.cache.write(Denomination.backoff_key('EUR'), true, expires_in: 5.minutes)
end
before['populated_history_eur'] = lambda do
  PopulatedFixture.build('history')
  User.first.update!(display_currency: 'EUR')
  Rails.cache.write('exchange_rate_USD_to_EUR', Result::Success.new(0.8), expires_in: 12.hours)
end
before['populated_fx_unavailable'] = lambda do
  PopulatedFixture.build('base')
  User.first.update!(display_currency: 'EUR')
  Rails.cache.write(Denomination.backoff_key('EUR'), true, expires_in: 5.minutes)
end
# The pending report's own key, with nothing on disk, with a refusal beside the CSV, and with the CSV.
%w[none refused ready].each do |state|
  before["populated_report_#{state}"] = lambda do
    PopulatedFixture.build('base')
    User.first.update!(tracker_settings: { 'pending_report' => { 'country' => 'DE', 'year' => 2025, 'report_scope' => 'broker' } })
    dir = Pathname(AppPaths.tmp).join('tax_reports')
    FileUtils.rm_rf(dir)
    FileUtils.mkdir_p(dir)
    path = Tax::GenerateReportJob.report_path(User.first.id, 'DE', 2025, 'broker')
    File.write("#{path}.refused.json", JSON.generate('reason' => 'tokenized_unsupported', 'symbols' => %w[TSLAx NVDAx])) if state == 'refused'
    File.write(path, "date,amount\n") if state == 'ready'
  end
end
# The owner's shape: three keys (one pending), a pending report with nothing on disk, euros, cash hidden.
before['populated_owner'] = lambda do
  before.fetch('populated_report_none').call
  User.first.update!(display_currency: 'EUR')
  Rails.cache.write('exchange_rate_USD_to_EUR', Result::Success.new(0.8), expires_in: 12.hours)
end
Pages.send(:remove_const, :BEFORE)
Pages.const_set(:BEFORE, before.freeze)

module D5PopulatedGrid
  def scenarios
    cases = {}
    add = lambda do |name, path, hook, attrs = {}|
      step = Pages.get(path).merge('action_snapshot' => true, 'expect' => 200, 'before' => "populated_#{hook}")
      cases["populated_#{name}"] = { 'user' => Pages.owner(attrs), 'd5_fixture' => 'alpaca', 'steps' => Pages.signed_in(step) }
    end
    I18n.available_locales.each do |locale|
      add.call("#{locale}", "/#{locale}/tracker", 'base')
      add.call("hidden_#{locale}", "/#{locale}/tracker", 'base', 'hide_balances' => true)
      add.call("notes_#{locale}", "/#{locale}/tracker", 'notes', 'tracker_settings' => { 'show_cash' => true })
    end
    add.call('cash', '/tracker', 'base', 'tracker_settings' => { 'show_cash' => true })
    add.call('history', '/tracker', 'history')
    add.call('history_cash', '/tracker', 'history', 'tracker_settings' => { 'show_cash' => true })
    add.call('history_hidden', '/tracker', 'history', 'hide_balances' => true)
    add.call('history_scope', '/tracker?exchange_id=2', 'history')
    add.call('history_long', '/tracker', 'long')
    add.call('history_eur', '/tracker', 'history_eur')
    add.call('rows', '/tracker', 'rows')
    add.call('rows_all', '/tracker?all=1', 'rows')
    add.call('rows_scope', '/tracker?exchange_id=2', 'rows')
    add.call('rows_hidden', '/tracker', 'rows', 'hide_balances' => true)
    add.call('scope_alpaca', '/tracker?exchange_id=1', 'base')
    add.call('scope_binance', '/tracker?exchange_id=2', 'base')
    add.call('scope_ibkr', '/tracker?exchange_id=3', 'base')
    add.call('scope_cash', '/tracker?exchange_id=2', 'base', 'tracker_settings' => { 'show_cash' => true })
    %w[many stale unpriced failed dead lone cold].each { |kind| add.call(kind, '/tracker', kind) }
    add.call('cold_hidden', '/tracker', 'cold', 'hide_balances' => true)
    add.call('cold_scope', '/tracker?exchange_id=2', 'cold')
    %w[EUR GBP CHF PLN].each { |currency| add.call("currency_#{currency}", '/tracker', "currency_#{currency}") }
    add.call('fx_unavailable', '/tracker', 'fx_unavailable')
    %w[GBP CHF PLN history].each { |kind| add.call("fx_unavailable_#{kind}", '/tracker', "fx_unavailable_#{kind}") }
    # A19 windows. `rows` holds 210 rows: ten on 09-01..09-06 and 200 AAPL buys on 09-07 (the unfiltered table).
    { 'from' => 'from=2026-09-06', 'to' => 'to=2026-09-06', 'both' => 'from=2026-09-02&to=2026-09-04',
      'day' => 'from=2026-09-06&to=2026-09-06', 'inverted' => 'from=2026-09-05&to=2026-09-02',
      'before_first' => 'from=2026-01-01&to=2026-08-31', 'scope' => 'exchange_id=2&from=2026-09-02&to=2026-09-06',
      'limited' => 'from=2026-09-01', 'all' => 'all=1&from=2026-09-06' }.each do |name, query|
      add.call("window_#{name}", "/tracker?#{query}", 'rows')
    end
    add.call('window_cold', '/tracker?from=2026-09-02&to=2026-09-03', 'cold')
    add.call('window_santiago', '/tracker?from=2026-09-06&to=2026-09-06', 'santiago', 'time_zone' => 'Santiago')
    add.call('window_magadan', '/tracker?from=2014-10-26&to=2014-10-26', 'magadan', 'time_zone' => 'Magadan')
    # A19 bad input. Rails parses the first three (Rust refuses: D-TRACKER-FILTER-ISO-ONLY) and raises on the rest
    # (D-TRACKER-FILTER-BAD-INPUT).
    { 'nov' => 'from=Nov%203', 'compact' => 'from=20260903', 'slashes' => 'from=11%2F09%2F2026' }.each do |name, query|
      add.call("bad_#{name}", "/tracker?#{query}", 'base')
    end
    { 'garbage' => 'from=garbage', 'impossible' => 'from=2026-02-30', 'month' => 'to=2026-11', 'from_array' => 'from[]=2026-09-01',
      'to_hash' => 'to[a]=x', 'exchange_array' => 'exchange_id[]=1' }.each do |name, query|
      add.call("bad_#{name}", "/tracker?#{query}", 'base')
      cases["populated_bad_#{name}"]['steps'].last.merge!('expect' => 500, 'rails_exception_status' => 500)
    end
    # Exchange.find(a Parameters) is RecordNotFound: a 404, served as Rails serves it.
    add.call('bad_exchange_hash', '/tracker?exchange_id[a]=1', 'base')
    cases['populated_bad_exchange_hash']['steps'].last.merge!('expect' => 404, 'rails_exception_status' => 404)
    %w[none refused ready].each { |state| add.call("report_#{state}", '/tracker', "report_#{state}") }
    add.call('owner', '/tracker', 'owner')
    cases
  end
end
Pages.singleton_class.prepend(D5PopulatedGrid)
command, root = ARGV
raise 'usage: grid|record DIR' unless %w[grid record].include?(command) && root

Pages.public_send(command, root)
if command == 'record'
  Dir[File.join(root, '*/rails.json')].each do |path|
    result = JSON.parse(File.read(path))
    answer = result.fetch('responses').last
    raise "network attempt in #{path}" unless result.fetch('network').empty?
    expect = JSON.parse(File.read(File.join(File.dirname(path), 'scenario.json'))).fetch('steps').last.fetch('expect', 200)
    raise "status #{answer['status']} in #{path}" unless answer.fetch('status') == expect
  end
  sources = %w[
    app/controllers/tracker_controller.rb app/models/tracker/figures.rb app/models/api_key.rb app/models/exchange.rb
    app/models/portfolio_snapshot.rb app/models/denomination.rb app/helpers/tracker_helper.rb app/helpers/application_helper.rb
    app/jobs/tax/generate_report_job.rb app/views/tracker/index.html.erb app/views/tracker/_exchange_filters.html.erb
    app/views/tracker/_tiles.html.erb app/views/tracker/_holdings.html.erb app/views/tracker/_holding_row.html.erb
    app/views/tracker/_sync_warnings.html.erb app/views/tracker/_key_fix_buttons.html.erb app/views/tracker/_report_refused.html.erb
    app/views/shared/_segmented.html.erb app/views/bots/_metrics_item.html.erb app/views/assets/_logo.html.erb
    app/views/tracker/_chart.html.erb app/views/tracker/_record.html.erb app/views/tracker/_transaction_row.html.erb
    app/views/tracker/_positions_row.html.erb app/helpers/bot_helper.rb app/models/account_transaction.rb
  ]
  File.write(File.join(root, 'sources.json'), "#{JSON.pretty_generate(sources.to_h { |p| [p, Digest::SHA256.file(Rails.root.join(p)).hexdigest] })}\n")
end
