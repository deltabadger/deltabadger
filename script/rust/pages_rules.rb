# W1: X0's fifteen page cells, replayed through the page grid with real cookies and CSRF.
# Rails remains the oracle; only the named differences in tests/pages.rs are removed.
module Pages
  module RulesGrid
    ADDRESS = 'bc1qx0destination0000000000000000000000'.freeze
    BASE = '/rules/withdrawals'.freeze
    COLUMNS = %w[id user_id type status exchange_id asset_id settings].freeze

    def scenarios
      super.merge(rules_scenarios)
    end

    def rule_step(method, path, form = {})
      step = method == 'GET' ? get(path) : post(path, form, 'csrf' => 'header', 'headers' => TURBO).merge('method' => method)
      step.merge('raw_rows' => { 'rules' => COLUMNS, 'rule_logs' => '*' }, 'may_mutate' => ['rules'],
                 'session' => ['withdrawal_rule_config'])
    end

    def rules_scenarios
      choose = ->(venue = '3') { [rule_step('GET', "#{BASE}/pick_exchange/new"),
        rule_step('POST', "#{BASE}/pick_exchange", 'bots_dca_single_asset[exchange_id]' => venue),
        rule_step('GET', "#{BASE}/add_api_key/new"), rule_step('GET', "#{BASE}/pick_asset/new"),
        rule_step('POST', "#{BASE}/pick_asset", 'bots_dca_single_asset[asset_id]' => '1'),
        rule_step('GET', "#{BASE}/add_api_key/new")] }
      wizard = ->(steps, fixture = 'wizard') { { 'user' => owner, 'rules_fixture' => fixture, 'steps' => signed_in(get('/bots'), *steps) } }
      one = ->(venue, status, settings = {}) { [{ 'venue' => venue, 'status' => status, 'settings' => settings }] }
      patch = ->(attrs) { rule_step('PATCH', "#{BASE}/1", attrs.transform_keys { |k| "rules_withdrawal[#{k}]" }) }
      cells = {
        'index_empty' => [[], rule_step('GET', '/rules')],
        'index_each_status' => [[*one.('Binance', 'created'), *one.('Kraken', 'scheduled'), *one.('Gemini', 'stopped'), *one.('Mexc', 'deleted')], rule_step('GET', '/rules')],
        'index_listing_failed' => [one.('Gemini', 'stopped'), rule_step('GET', '/rules')],
        'tile_start_created' => [one.('Binance', 'created'), patch.('status' => 'scheduled')],
        'tile_start_stopped' => [one.('Binance', 'stopped'), patch.('status' => 'scheduled')],
        'tile_start_invalid' => [one.('Binance', 'stopped', 'max_fee_percentage' => nil), patch.('status' => 'scheduled')],
        'tile_stop_scheduled' => [one.('Binance', 'scheduled'), patch.('status' => 'stopped')],
        'tile_settings_scheduled' => [one.('Binance', 'scheduled'), patch.('withdrawal_percentage' => '40', 'max_interval' => '3')],
        'tile_settings_stopped' => [one.('Gemini', 'stopped'), patch.('withdrawal_percentage' => '40')],
        'tile_address_not_listed' => [one.('Gemini', 'stopped'), patch.('address' => 'bc1qnotlisted')],
        'tile_address_listed' => [one.('Gemini', 'stopped'), patch.('address' => ADDRESS)],
        'confirm_destroy' => [one.('Binance', 'stopped'), rule_step('GET', "#{BASE}/1/confirm_destroy").merge('headers' => { 'Turbo-Frame' => 'modal' })],
        'destroy_scheduled' => [one.('Binance', 'scheduled'), rule_step('DELETE', "#{BASE}/1")],
        'destroy_created' => [one.('Binance', 'created'), rule_step('DELETE', "#{BASE}/1")],
        'post_withdrawals' => [[], rule_step('POST', BASE).merge('expect' => 404)]
      }
      cells.to_h do |name, (rules, step)|
        ["rules_#{name}", { 'user' => owner, 'rules_fixture' => name, 'rules_x0' => true, 'withdrawal_rules' => rules,
                             'steps' => signed_in(get('/bots'), step) }]
      end.merge(
         'rules_wizard_gemini' => wizard.([*choose.(), rule_step('GET', "#{BASE}/confirm_settings/new"),
          rule_step('POST', "#{BASE}/confirm_settings/preview", 'withdrawal_percentage' => '50', 'threshold_type' => 'min_amount', 'min_amount' => '0.2'),
          rule_step('POST', "#{BASE}/confirm_settings", 'withdrawal_percentage' => '50', 'threshold_type' => 'min_amount',
                    'min_amount' => '0.2', 'max_interval' => '7', 'network' => 'bitcoin'), rule_step('GET', '/rules')]),
        'rules_wizard_address' => wizard.([*choose.(), rule_step('GET', "#{BASE}/add_address/new"),
          rule_step('POST', "#{BASE}/add_address", 'address' => 'not-listed'),
          rule_step('POST', "#{BASE}/add_address", 'address' => ADDRESS), rule_step('GET', "#{BASE}/confirm_settings/new")]),
        'rules_wizard_no_key' => wizard.([*choose.()], 'no_key'),
        'rules_wizard_no_addresses' => wizard.([*choose.(), rule_step('GET', "#{BASE}/add_address/new")], 'empty_addresses'),
        'rules_tile_combined_invalid_start' => wizard.([patch.('status' => 'scheduled', 'max_fee_percentage' => '-1')]).merge('withdrawal_rules' => one.('Binance', 'stopped')),
        'rules_tile_combined_invalid_stop' => wizard.([patch.('status' => 'stopped', 'max_fee_percentage' => '-1')]).merge('withdrawal_rules' => one.('Binance', 'scheduled')),
        'rules_wizard_duplicate' => wizard.([*choose.(), rule_step('POST', "#{BASE}/confirm_settings", 'network' => 'bitcoin')]).merge('withdrawal_rules' => one.('Gemini', 'stopped')),
        'rules_wizard_revive' => wizard.([*choose.(), rule_step('POST', "#{BASE}/confirm_settings", 'network' => 'bitcoin'),
          rule_step('GET', '/rules')]).merge('withdrawal_rules' => one.('Gemini', 'deleted')),
        'rules_wizard_unlisted_save' => wizard.([*choose.(), rule_step('POST', "#{BASE}/confirm_settings", 'address' => 'not-listed')]),
        'rules_wizard_no_fee' => wizard.([*choose.(), rule_step('GET', "#{BASE}/confirm_settings/new"),
          rule_step('POST', "#{BASE}/confirm_settings/preview", 'threshold_type' => 'min_amount', 'min_amount' => '0.1')], 'no_fee'),
        'rules_wizard_failed_listing' => wizard.([*choose.(), rule_step('GET', "#{BASE}/add_address/new")], 'index_listing_failed'),
        'rules_wizard_exchange' => { 'user' => owner, 'rules_fixture' => 'wizard', 'steps' => signed_in(get('/bots'),
          rule_step('GET', "#{BASE}/pick_exchange/new"), rule_step('GET', "/de#{BASE}/pick_exchange/new")) },
        'rules_wizard_prerequisites' => { 'user' => owner, 'rules_fixture' => 'wizard', 'steps' => signed_in(get('/bots'),
          *%w[pick_asset add_api_key add_address confirm_settings].map { |step| rule_step('GET', "#{BASE}/#{step}/new") }) }
      )
    end

    def action_fixture(scenario)
      super
      return unless scenario['rules_fixture']

      asset = Asset.create!(symbol: 'BTC', external_id: 'bitcoin', name: 'Bitcoin', category: 'Cryptocurrency', color: scenario['rules_x0'] ? nil : '#f7931a')
      quote = Asset.create!(symbol: 'USD', external_id: 'usd', name: 'US Dollar', category: 'Fiat', color: '#85bb65')
      %w[Binance Kraken Gemini Mexc BinanceUs].each do |kind|
        name = { 'Mexc' => 'MEXC', 'BinanceUs' => 'Binance.US' }.fetch(kind, kind)
        exchange = "Exchanges::#{kind}".constantize.create!(name:, maker_fee: '0.1', taker_fee: '0.1')
        ApiKey.create!(user: User.first, exchange:, key_type: :withdrawal, status: :correct,
                       key: { 'index_listing_failed' => 'rules-failed', 'empty_addresses' => 'rules-empty' }.fetch(scenario['rules_fixture'], 'rules-key'), secret: Base64.strict_encode64('rules-secret')) unless scenario['rules_fixture'] == 'no_key'
        ExchangeAsset.create!(exchange:, asset:, withdrawal_fee: (scenario['rules_x0'] || scenario['rules_fixture'] == 'no_fee') ? nil : '0.0001',
                              withdrawal_fee_updated_at: Time.iso8601(AT), withdrawal_chains: [])
        ExchangeAsset.create!(exchange:, asset: quote)
        Ticker.create!(exchange:, base: kind == 'Kraken' ? 'XBT' : 'BTC', quote: 'USD', base_asset: asset, quote_asset: quote, ticker: 'BTCUSD',
                       available: true, minimum_base_size: '0.00001', minimum_quote_size: '1', base_decimals: 8, quote_decimals: 2, price_decimals: 2)
      end
      scenario.fetch('withdrawal_rules', []).each do |spec|
        rule = Rules::Withdrawal.create!(user: User.first, exchange: Exchange.find_by!(type: "Exchanges::#{spec['venue']}"), asset:,
          address: ADDRESS, status: spec['status'], settings: {
            'withdrawal_percentage' => '100', 'threshold_type' => 'fee_percentage', 'max_fee_percentage' => '1',
            **(spec['venue'] == 'Mexc' ? { 'network' => 'BTC' } : {})
          }.merge(spec.fetch('settings', {})).compact)
        next unless scenario['rules_fixture'] == 'index_each_status' && spec['venue'] == 'Kraken'

        { pending: 'Balance 0.005 BTC below minimum 0.01 BTC', failed: 'Withdrawal failed: You have insufficient balance.',
          transient: 'Failed to fetch balance — temporary exchange issue, retrying on the next cycle', success: 'Withdrew 0.9999 BTC' }
          .each_with_index { |(status, message), i| rule.rule_logs.create!(status:, message:, details: { 'free_balance' => '1.0' }, created_at: Time.iso8601(AT) - (4 - i).hours) }
      end
    end

    def record(root)
      WebMock.enable!
      WebMock.stub_request(:post, 'https://api.gemini.com/v1/approvedAddresses/account/bitcoin').to_return do |request|
        failed = request.headers['X-Gemini-Apikey'] == 'rules-failed'
        { status: failed ? 503 : 200, body: (failed ? { 'result' => 'error', 'reason' => 'System' } : {
          'approvedAddresses' => request.headers['X-Gemini-Apikey'] == 'rules-empty' ? [] : [{ 'network' => 'bitcoin', 'address' => ADDRESS, 'label' => 'cold', 'status' => 'active' }]
        }).to_json, headers: { 'Content-Type' => 'application/json' } }
      end
      super
    end
  end
  singleton_class.prepend(RulesGrid)
end
