# Current fiat rates reach Rails through its real Faraday/JSON/currency layers.
D5_TRACKER_LIBRARY = true
FIGURES_LIBRARY = true
require Rails.root.join('script/rust/pages_tracker')
require Rails.root.join('script/rust/figures')
Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(ScriptedMarket::Adapter)

D5_FX_RATES = { 'usd' => { 'value' => 100.0 }, 'eur' => { 'value' => 80.0 }, 'gbp' => { 'value' => 50.0 },
                'chf' => { 'value' => 90.0 }, 'pln' => { 'value' => 400.0 } }.freeze
fx_before = Pages::BEFORE.dup
%w[EUR GBP CHF PLN].each do |currency|
  fx_before["fx_#{currency}"] = lambda do
    User.first.update!(display_currency: currency)
    Rails.cache.clear
    ScriptedMarket.http = { 'GET 127.0.0.1:1/api/v1/exchange_rates' => { 'body' => { 'data' => D5_FX_RATES } } }
    ScriptedMarket.requests = []
    ScriptedMarket.gaps = []
  end
end
fx_before['fx_integer'] = lambda do
  User.first.update!(display_currency: 'EUR')
  Rails.cache.clear
  data = { 'usd' => { 'value' => 100 }, 'eur' => { 'value' => 80 } }
  ScriptedMarket.http = { 'GET 127.0.0.1:1/api/v1/exchange_rates' => { 'body' => { 'data' => data } } }
  ScriptedMarket.requests = []
  ScriptedMarket.gaps = []
end
# Preserve the review’s seventeenth digit on the wire; Rails/Oj JSON.generate rounds it.
{ 'review_clear' => [64_123.456, 55_210.987], 'review_precision' => [1.0, 1.0000000000000002],
  'review_reverse' => [1.0000000000000002, 1.0], 'review_zero' => [100.0, 0.0],
  'review_negative' => [-100.0, -80.0] }.each do |name, (usd, eur)|
  fx_before["fx_#{name}"] = lambda do
    User.first.update!(display_currency: 'EUR')
    HistoricalPrice.where(asset: 'BTC', currency: 'USD').update_all(price: '1') if name == 'review_clear'
    Rails.cache.clear
    wire = %({"data":{"usd":{"value":#{usd}},"eur":{"value":#{eur}}}})
    ScriptedMarket.http = { 'GET 127.0.0.1:1/api/v1/exchange_rates' => { 'body' => wire } }
    ScriptedMarket.requests = []
    ScriptedMarket.gaps = []
  end
end
{ 'string' => ['100.0', '80.0'], 'string_garbage' => ['100.0', '12abc'],
  'string_empty' => ['100.0', ''] }.each do |name, (usd, eur)|
  fx_before["fx_#{name}"] = lambda do
    User.first.update!(display_currency: 'EUR')
    Rails.cache.clear
    data = { 'usd' => { 'value' => usd }, 'eur' => { 'value' => eur } }
    ScriptedMarket.http = { 'GET 127.0.0.1:1/api/v1/exchange_rates' => { 'body' => { 'data' => data } } }
    ScriptedMarket.requests = []
    ScriptedMarket.gaps = []
  end
end
Pages.send(:remove_const, :BEFORE)
Pages.const_set(:BEFORE, fx_before.freeze)
module D5FxGrid
  def scenarios
    source = super
    cases = %w[EUR GBP CHF PLN].to_h do |currency|
      scenario = Marshal.load(Marshal.dump(source.fetch('d5_price_12.5')))
      scenario['steps'].last['before'] = "fx_#{currency}"
      scenario['steps'].last['form']['price'] = { 'EUR' => '8', 'GBP' => '5', 'CHF' => '9', 'PLN' => '40' }.fetch(currency)
      ["d5_fx_#{currency}", scenario]
    end
    integer = Marshal.load(Marshal.dump(source.fetch('d5_price_12.5')))
    integer['steps'].last['before'] = 'fx_integer'
    integer['steps'].last['form']['price'] = '80'
    cases['d5_fx_integer'] = integer
    %w[review_clear review_precision review_reverse review_zero review_negative].each do |name|
      scenario = Marshal.load(Marshal.dump(source.fetch(name == 'review_clear' ? 'd5_price_clear' : 'd5_price_12.5')))
      scenario['steps'].last['before'] = "fx_#{name}"
      scenario['steps'].last['form']['price'] = name == 'review_clear' ? '' : '100000000000000.02'
      cases["d5_fx_#{name}"] = scenario
    end
    %w[string string_garbage string_empty].each do |name|
      scenario = Marshal.load(Marshal.dump(source.fetch('d5_price_12.5')))
      scenario['steps'].last['before'] = "fx_#{name}"
      scenario['steps'].last['form']['price'] = name == 'string_garbage' ? '12' : '80'
      cases["d5_fx_#{name}"] = scenario
    end
    I18n.available_locales.each do |locale|
      scenario = Marshal.load(Marshal.dump(source.fetch("d5_price_locale_#{locale}")))
      scenario['steps'].last['before'] = 'fx_EUR'
      scenario['steps'].last['form']['price'] = '8'
      cases["d5_fx_locale_#{locale}"] = scenario
    end
    %w[link_missing price_hidden price_clear price_small].each do |name|
      scenario = Marshal.load(Marshal.dump(source.fetch("d5_#{name}")))
      scenario['steps'].last['before'] = 'fx_EUR'
      cases["d5_fx_#{name}"] = scenario
    end
    # The successful link followed by unlink exercises both returned transaction rows.
    scenario = Marshal.load(Marshal.dump(source.fetch('d5_link_unlink')))
    scenario['steps'].last['before'] = 'fx_EUR'
    cases['d5_fx_link_unlink'] = scenario
    cases
  end
end
Pages.singleton_class.prepend(D5FxGrid)
command, root = ARGV
raise 'usage: grid|record DIR' unless %w[grid record].include?(command) && root

Pages.public_send(command, root)
if command == 'record'
  Dir[File.join(root, '*/rails.json')].each do |path|
    result = JSON.parse(File.read(path))
    raise 'unexpected real network' unless result.fetch('network').empty?
    raise 'secret in response' if result.fetch('responses').any? { |r| r.fetch('body').include?('synthetic-d5-market-token') }
    raise 'unexpected FX status' unless result.fetch('responses').last.fetch('status') == 200
  end
end
