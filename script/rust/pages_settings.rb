# S1 extends the existing page oracle; production controllers/models stay unchanged.
require 'webmock'

module Pages
  SETTINGS_TABLES = %w[users api_keys bots account_transactions connected_clients oauth_applications oauth_access_tokens oauth_access_grants
                       wash_sale_locks app_configs].freeze
  SECRET_COLUMNS = { 'users' => %w[otp_secret_key], 'api_keys' => ApiKey::CREDENTIAL_ATTRIBUTES.map(&:to_s),
                     'app_configs' => %w[value] }.freeze

  def self.settings_rows
    SETTINGS_TABLES.to_h do |table|
      rows = ActiveRecord::Base.connection.select_all("SELECT * FROM #{table} ORDER BY id").to_a
      rows.each do |row|
        if table == 'users'
          password = [PASSWORD, 'Another-horse-7', 'Correcthorse9Ż', 'Correcthorse١!', "Correct\nhorse-9", '   '].find { |value| BCrypt::Password.new(row['encrypted_password']) == value }
          raise 'settings oracle: unrecognized password hash' unless password

          row['encrypted_password'] = { 'verifies' => password }
          if row['confirmation_token']
            raise 'settings oracle: invalid confirmation token' unless row['confirmation_token'].match?(/\A[A-Za-z0-9_-]{20}\z/)

            row['confirmation_token'] = '<valid confirmation token>'
          end
        end
        decrypted!(table, row)
        %w[mcp_settings rest_settings tracker_settings settings transient_data mcp_tools rest_tools].each do |column|
          row[column] = JSON.parse(row[column]) if row[column].is_a?(String)
        end
      end
      [table, rows]
    end
  end

  # The credentials of a row as the model reads them: snapshots compare what a key is, never its ciphertext.
  def self.decrypted!(table, row)
    SECRET_COLUMNS.fetch(table, []).each do |column|
      next if row[column].nil?

      # AppConfig encrypts only selected names, handled by its accessor rather than a blanket mask.
      if table == 'app_configs'
        row[column] = AppConfig.get(row['key'])
      else
        record = table == 'users' ? User.find(row['id']) : ApiKey.find(row['id'])
        row[column] = record.public_send(column)
      end
    end
    row
  end

  IBKR_KEYS = JSON.parse(File.read(Rails.root.join('rust/tests/fixtures/ibkr_test_keys.json'))).freeze
  IBKR_VERDICTS = JSON.parse(File.read(Rails.root.join('rust/tests/fixtures/ibkr_key_verdicts.json')))['ibkr'].freeze
  IBKR_DH = IBKR_VERDICTS['dh_param']
  # The access token an activation submits picks IBKR's answer (rust/tests/pages.rs mounts the same): the recorded mint and
  # brokerage session of ibkr_key_verdicts.json's one_account case, then its accounts, or a brokerage session that fails.
  IBKR_ACCOUNTS = { 'tok-pending' => { 'accounts' => [] }, 'tok-correct' => { 'accounts' => ['U1234567'], 'selectedAccount' => 'U1234567' } }.freeze
  IBKR_FAILING = 'tok-failure'
  # A17-1: the CoinGecko keys the steps submit (dummies; CoinGecko is stubbed), and its coins/markets answer: the Top Coins
  # ranking the wizard's crypto indices store (pages_wizard.rb index_fixture), by market cap.
  CG_KEY = 'CG-test-key'
  CG_BAD = 'CG-bad-key'
  CG_COINS = %w[btc eth solana ada-coin avax-coin dot-coin link-coin].each_with_index.map do |id, rank|
    { 'id' => id, 'symbol' => id.split('-').first, 'name' => id.split('-').first.upcase, 'image' => '', 'current_price' => 10 - rank,
      'market_cap' => (7 - rank) * 1_000_000_000.0, 'market_cap_rank' => rank + 1, 'circulating_supply' => 1_000_000 }
  end.freeze

  module SettingsGrid
    def scenarios
      super.merge(settings_scenarios)
    end

    def action_fixture(scenario)
      super
      # 'key_sync_error': what the last sync of the stored keys recorded (a missing permission, for one).
      ApiKey.update_all(last_sync_error: scenario['key_sync_error']) if scenario['key_sync_error']
      return unless scenario['settings_fixture']

      owner = User.first
      owner.update_columns(otp_secret_key: scenario['missing_otp'] ? nil : OTP_SEED) # stable QR/code bytes, no secret rotation
      case scenario['settings_fixture']
      when 'keys', 'keys_warnings'
        exchange = Exchanges::Alpaca.first || Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: 0, taker_fee: 0)
        ApiKey.create!(user: owner, exchange:, key_type: :trading, status: scenario.fetch('key_status', 'correct'),
                       key: 'previous-key', secret: 'previous-secret', passphrase: scenario.fetch('key_mode', 'paper'))
        if scenario['settings_fixture'] == 'keys_warnings'
          kraken = Exchanges::Kraken.create!(name: 'Kraken', maker_fee: 0, taker_fee: 0)
          ApiKey.create!(user: owner, exchange: kraken, key_type: :read_only, status: :incorrect,
                         key: 'warning-key', secret: 'warning-secret', last_sync_error: 'EAPI:Invalid key')
        end
        if scenario['legacy_history']
          key = owner.api_keys.find_by!(exchange:)
          key.update_columns(created_at: Time.utc(2026, 1, 1))
          AccountTransaction.create!(user: owner, exchange:, api_key: key, base_amount: 0.25, base_currency: 'BTC', entry_type: 0,
                                     transacted_at: Time.utc(2026, 9, 1))
        end
      when 'ibkr'
        # The connect page (Settings::IbkrConnectionsController) over the one IBKR key 'ibkr_key' describes: the throwaway test
        # material of rust/tests/fixtures/ibkr_test_keys.json with the ffdhe2048 group the recorded IBKR answers were made with.
        exchange = Exchanges::Ibkr.create!(name: 'Interactive Brokers', maker_fee: 0, taker_fee: 0)
        if (spec = scenario['ibkr_key'])
          attrs = { user: owner, exchange:, key_type: :trading, status: spec.fetch('status', 'pending_validation') }
          # 'generating': the row Rails' create leaves until Ibkr::GenerateConnectionKeysJob has run.
          attrs.merge!(rsa_signature_key: IBKR_KEYS['signature_key'], rsa_encryption_key: IBKR_KEYS['encryption_key'], dh_param: IBKR_DH) unless spec['generating']
          if spec['credentials']
            attrs.merge!(key: IBKR_KEYS['consumer_key'], secret: IBKR_KEYS['access_token_secret'], access_token: IBKR_KEYS['access_token'],
                         ibkr_realm: IBKR_KEYS['realm'])
          end
          key = ApiKey.create!(attrs)
          key.update_columns(updated_at: Time.iso8601(spec['updated_at'])) if spec['updated_at']
        end
      when 'venues'
        # D5b: every venue the tracker's picker may list. Gemini is not available; BitMart is retired but still available.
        [['Alpaca', 'Alpaca'], ['Binance', 'Binance'], ['BinanceUs', 'Binance.US'], ['Ibkr', 'Interactive Brokers'], ['Kraken', 'Kraken'],
         ['Bitmart', 'BitMart'], ['Gemini', 'Gemini', false], ['Hyperliquid', 'Hyperliquid'], ['Bitget', 'Bitget']].each do |klass, name, available|
          "Exchanges::#{klass}".constantize.create!(name:, maker_fee: 0, taker_fee: 0, available: available != false)
        end
      when 'clients', 'clients_following'
        other = User.create!(name: 'Other', email: 'other@example.com', password: PASSWORD, confirmed_at: Time.current)
        application = Doorkeeper::Application.create!(name: 'Owner client', uid: 'settings-client', secret: '',
                                                      redirect_uri: 'http://localhost:9911/callback', confidential: false, scopes: 'mcp api')
        [owner, other].each do |user|
          Doorkeeper::AccessToken.create!(application:, resource_owner_id: user.id, token: "settings-token-#{user.id}",
                                          refresh_token: "settings-refresh-#{user.id}", expires_in: 1, scopes: 'mcp')
          ConnectedClient.create!(user:, oauth_application: application, mcp_tools: AppConfig::MCP_TOOL_GROUPS['read'], rest_tools: [])
        end
        # A client connected after consent stopped asking: no stored lists, it follows the owner's switches.
        ConnectedClient.where(user: owner).update_all(mcp_tools: nil, rest_tools: nil) if scenario['settings_fixture'] == 'clients_following'
      end
    end

    def record(root)
      WebMock.enable!
      WebMock.disable_net_connect!
      # Every request a stub answers goes to the step's venue transcript; one no stub answers is the network.
      WebMock.after_request { |request, _response| VENUE << venue_request(request) }
      WebMock.globally_stub_request(:after_local_stubs) do |request|
        NETWORK << request.uri.to_s
        nil
      end
      # Validations call the real client and model against a deterministic paper account response.
      WebMock.stub_request(:get, 'https://paper-api.alpaca.markets/v2/positions').to_return(status: 200, body: '[]',
                                                                                            headers: { 'Content-Type' => 'application/json' })
      WebMock.stub_request(:get, 'https://paper-api.alpaca.markets/v2/clock').to_return(status: 200, body: '{"is_open":true}',
                                                                                        headers: { 'Content-Type' => 'application/json' })
      WebMock.stub_request(:get, 'https://paper-api.alpaca.markets/v2/account')
             .to_return(status: 200, body: '{"status":"ACTIVE","cash":"0"}', headers: { 'Content-Type' => 'application/json' })
      WebMock.stub_request(:get, 'https://paper-api.alpaca.markets/v2/account').with(headers: { 'APCA-API-KEY-ID' => 'incorrect-key' }).to_return(
        status: 401, body: '{}'
      )
      WebMock.stub_request(:get, 'https://paper-api.alpaca.markets/v2/account').with(headers: { 'APCA-API-KEY-ID' => 'failure-key' }).to_return(
        status: 503, body: '{"message":"unavailable"}'
      )
      WebMock.stub_request(:get, 'https://paper-api.alpaca.markets/v2/positions')
             .with(headers: { 'APCA-API-KEY-ID' => 'positions-bad' }).to_return(status: 401, body: '{"message":"unauthorized"}')
      WebMock.stub_request(:get, 'https://paper-api.alpaca.markets/v2/account')
             .with(headers: { 'APCA-API-KEY-ID' => 'r9-no-cash' })
             .to_return(status: 200, body: '{"status":"ACTIVE"}', headers: { 'Content-Type' => 'application/json' })
      WebMock.stub_request(:get, 'https://paper-api.alpaca.markets/v2/account')
             .with(headers: { 'APCA-API-KEY-ID' => 'r9-null-cash' })
             .to_return(status: 200, body: '{"status":"ACTIVE","cash":null}', headers: { 'Content-Type' => 'application/json' })
      WebMock.stub_request(:get, 'https://paper-api.alpaca.markets/v2/positions')
             .with(headers: { 'APCA-API-KEY-ID' => 'r9-unnamed-position' })
             .to_return(status: 200, body: '[{"asset_class":"us_equity","qty":"2"}]', headers: { 'Content-Type' => 'application/json' })
      # 3.0 plan A4: an open order's cancel (Clients::Alpaca#cancel_order), by order id.
      { 'cancel-ok' => [204, ''], 'cancel-refused' => [422, '{"code":42210000,"message":"order is not cancelable"}'],
        'cancel-key' => [401, '{"message":"unauthorized"}'] }.each do |id, (status, body)|
        WebMock.stub_request(:delete, %r{\Ahttps://(paper-)?api\.alpaca\.markets/v2/orders/#{id}\z})
               .to_return(status:, body:, headers: { 'Content-Type' => 'application/json' })
      end
      ibkr_stubs
      coingecko_stubs
      a18_stubs
      super
    ensure
      WebMock.reset!
      WebMock.disable!
    end

    # CoinGecko's side of a key check (Coingecko#get_top_coins_by_market_cap through Clients::Coingecko): the plan ping on the
    # paid API refuses every key (a demo key), the demo API answers the coins, a key it refuses with 401. rust/tests/pages.rs
    # mounts the same.
    def coingecko_stubs
      json = { 'Content-Type' => 'application/json' }
      WebMock.stub_request(:get, 'https://pro-api.coingecko.com/api/v3/ping').to_return(status: 401, body: '{"error":"invalid"}', headers: json)
      markets = %r{\Ahttps://api\.coingecko\.com/api/v3/coins/markets}
      WebMock.stub_request(:get, markets).to_return(status: 200, body: CG_COINS.to_json, headers: json)
      WebMock.stub_request(:get, markets).with(headers: { 'x-cg-demo-api-key' => CG_BAD })
             .to_return(status: 401, body: '{"status":{"error_code":10002}}', headers: json)
    end

    # A18: the launchpad's claim (Clients::Launchpad#claim at LAUNCHPAD_URL's default) by code, and Alpaca's live account read.
    # rust/tests/pages.rs mounts the same.
    def a18_stubs
      json = { 'Content-Type' => 'application/json' }
      claim = 'https://deltabadger.com/api/claim'
      market = { 'url' => 'http://data-api.invalid', 'token' => 'platform-token' }
      WebMock.stub_request(:post, claim).with(body: { 'code' => 'dbc_ok' })
             .to_return(status: 200, headers: json, body: { 'identity' => { 'name' => 'Owner', 'email' => 'owner@example.com' }, 'market_data' => market,
                                                            'proxies' => { 'Binance' => 'http://proxy.invalid:8080', 'kraken' => 'http://proxy.invalid:8081' } }.to_json)
      WebMock.stub_request(:post, claim).with(body: { 'code' => 'dbc_no_proxies' })
             .to_return(status: 200, headers: json, body: { 'identity' => {}, 'market_data' => market }.to_json)
      WebMock.stub_request(:post, claim).with(body: { 'code' => 'dbc_bad' }).to_return(status: 401, headers: json, body: '{"error":"invalid"}')
      WebMock.stub_request(:get, 'https://api.alpaca.markets/v2/account')
             .to_return(status: 200, body: '{"status":"ACTIVE","cash":"0"}', headers: json)
      # A20: the open-orders refresh's read of a waiting order (pages.rb broadcasts_fetch_open_orders_waiting), refused.
      WebMock.stub_request(:get, 'https://paper-api.alpaca.markets/v2/orders/refresh-1').to_return(status: 503, body: '{"message":"unavailable"}', headers: json)
    end

    # The plan Clients::Coingecko detected and the client MarketData holds are memoised for the process: each scenario starts
    # without them, so its first CoinGecko call pings as a fresh process does.
    def connect(dir)
      Clients::Coingecko.instance_variable_set(:@detect_plan, nil)
      MarketData.instance_variable_set(:@coingecko, nil)
      super
    end

    # IBKR's side of an activation (ApiKey#validate_credentials! through Clients::Ibkr::Session): Rails' DH random fixed as
    # record_key_verdicts.rb fixes it, so the recorded mint answer is the one that verifies.
    def ibkr_stubs
      dh_random = IBKR_VERDICTS['dh_random']
      Clients::Ibkr::Oauth.singleton_class.prepend(Module.new { define_method(:dh_random) { dh_random } })
      one = IBKR_VERDICTS['cases'].find { |c| c['case'] == 'one_account' }['responses']
      answer = ->(recorded) { { status: recorded['status'], body: recorded['body'], headers: { 'Content-Type' => recorded['content_type'] } } }
      WebMock.stub_request(:post, 'https://api.ibkr.com/v1/api/oauth/live_session_token').to_return(answer.(one[0]))
      WebMock.stub_request(:post, 'https://api.ibkr.com/v1/api/iserver/auth/ssodh/init').to_return(answer.(one[1]))
      WebMock.stub_request(:post, 'https://api.ibkr.com/v1/api/iserver/auth/ssodh/init')
             .with(headers: { 'Authorization' => /oauth_token="#{IBKR_FAILING}"/ }).to_return(status: 500, body: '', headers: { 'Content-Type' => 'text/plain' })
      IBKR_ACCOUNTS.each do |token, body|
        WebMock.stub_request(:get, 'https://api.ibkr.com/v1/api/iserver/accounts').with(headers: { 'Authorization' => /oauth_token="#{token}"/ })
               .to_return(status: 200, body: body.to_json, headers: { 'Content-Type' => 'application/json' })
      end
    end
  end
  singleton_class.prepend(SettingsGrid)

  def self.settings_write(action, form, status = 200, opts = {})
    post("/settings/#{action}", form, 'csrf' => 'header', 'headers' => TURBO,
                                      'expect' => status, 'settings_snapshot' => true).merge(opts)
  end

  def self.settings_signed(*steps)
    [get('/login'), login, get('/settings/account')] + steps
  end

  def self.settings_case(*steps, fixture: 'account', user: owner)
    { 'settings_fixture' => fixture, 'user' => user, 'steps' => settings_signed(*steps) }.tap do |scenario|
      scenario['extra_users'] = [owner('name' => 'Admin', 'email' => 'admin@example.com')] unless user['admin']
    end
  end

  def self.settings_scenarios
    grid = {}
    I18n.available_locales.each do |locale|
      %w[account api_keys connect mcp api].each do |page|
        grid["settings_page_#{page}_#{locale}"] = settings_case(get("/#{locale}/settings/#{page}"))
        grid["settings_frame_#{page}_#{locale}"] = settings_case(get("/#{locale}/settings/#{page}", 'Turbo-Frame' => 'modal'))
      end
    end
    %w[account api_keys connect mcp api edit_two_fa].each do |page|
      grid["settings_signed_out_#{page}"] = { 'steps' => [get("/settings/#{page}")] }
      grid["settings_nonadmin_#{page}"] = settings_case(get("/settings/#{page}"), user: owner('admin' => false))
    end
    {
      'name_valid' => ['update_name', { 'user[name]' => 'New Owner' }, 200],
      'name_invalid' => ['update_name', { 'user[name]' => '1' }, 422],
      'name_blank' => ['update_name', { 'user[name]' => '' }, 422],
      'name_unicode' => ['update_name', { 'user[name]' => 'Żółć Жук' }, 200],
      'name_nbsp' => ['update_name', { 'user[name]' => "Alice\u00a0Smith" }, 422],
      'name_em_space' => ['update_name', { 'user[name]' => "Alice\u2003Smith" }, 422],
      'time_zone_valid' => ['update_time_zone', { 'user[time_zone]' => 'Warsaw' }, 200],
      'time_zone_blank' => ['update_time_zone', { 'user[time_zone]' => '' }, 200],
      'time_zone_invalid' => ['update_time_zone', { 'user[time_zone]' => 'invalid' }, 422],
      'locale_valid' => ['update_locale', { 'user[locale]' => 'de' }, 200],
      'locale_blank' => ['update_locale', { 'user[locale]' => '' }, 200],
      'locale_invalid' => ['update_locale', { 'user[locale]' => 'zz' }, 422],
      'email_valid' => ['update_email', { 'user[email]' => 'next@example.com', 'user[current_password]' => PASSWORD }, 200],
      'email_wrong_password' => ['update_email', { 'user[email]' => 'next@example.com', 'user[current_password]' => 'wrong' }, 422],
      'email_invalid' => ['update_email', { 'user[email]' => 'invalid', 'user[current_password]' => PASSWORD }, 422],
      'password_valid' => ['update_password',
                           { 'user[password]' => 'Another-horse-7', 'user[password_confirmation]' => 'Another-horse-7', 'user[current_password]' => PASSWORD }, 200], # rubocop:disable Layout/LineLength -- Literal parity request values.
      'password_wrong_current' => ['update_password',
                                   { 'user[password]' => 'Another-horse-7', 'user[password_confirmation]' => 'Another-horse-7', 'user[current_password]' => 'wrong' }, 422], # rubocop:disable Layout/LineLength -- Literal parity request values.
      'password_mismatch' => ['update_password',
                              { 'user[password]' => 'Another-horse-7', 'user[password_confirmation]' => 'mismatch', 'user[current_password]' => PASSWORD }, 422], # rubocop:disable Layout/LineLength -- Literal parity request values.
      'password_simple' => ['update_password',
                            { 'user[password]' => 'simple', 'user[password_confirmation]' => 'simple', 'user[current_password]' => PASSWORD }, 422],
      'password_blank' => ['update_password', { 'user[password]' => '', 'user[password_confirmation]' => '', 'user[current_password]' => PASSWORD },
                           200]
    }.each do |name, (action, form, status)|
      grid["settings_write_#{name}"] = settings_case(settings_write(action, { '_method' => 'patch' }.merge(form), status))
      grid["settings_csrf_#{name}"] = settings_case(settings_write(action, { '_method' => 'patch' }.merge(form), 302, 'csrf' => 'none'))
      grid["settings_origin_#{name}"] = settings_case(settings_write(action, { '_method' => 'patch' }.merge(form), 302,
                                                                     'headers' => TURBO.merge('Origin' => 'http://foreign.example')))
    end
    # The way back to the default locale, from the prefixed page the switch was made on: the stream
    # must reload the unprefixed page whole, or the permanent menu keeps its /de/ links.
    grid['settings_write_locale_back_to_default'] =
      settings_case(get('/de/settings/account'),
                    settings_write('update_locale', { '_method' => 'patch', 'user[locale]' => 'en' }).merge('path' => '/de/settings/update_locale'),
                    user: owner('locale' => 'de'))
    # 2FA opens inside its own row: the edit form answers the two_fa frame.
    two_fa = get('/settings/edit_two_fa', 'Turbo-Frame' => 'two_fa')
    grid['settings_write_locale_page_behind'] =
      settings_case(settings_write('update_locale', { '_method' => 'patch', 'user[locale]' => 'de' }, 200,
                                   'headers' => TURBO.merge('Referer' => 'http://localhost:3000/tracker')))
    grid['settings_write_locale_page_behind_prefixed'] =
      settings_case(settings_write('update_locale', { '_method' => 'patch', 'user[locale]' => 'en' }, 200,
                                   'path' => '/de/settings/update_locale', 'headers' => TURBO.merge('Referer' => 'http://localhost:3000/de/bots')),
                    user: owner('locale' => 'de'))
    grid['settings_write_locale_page_behind_protocol_relative'] =
      settings_case(settings_write('update_locale', { '_method' => 'patch', 'user[locale]' => 'de' }, 200,
                                   'headers' => TURBO.merge('Referer' => 'http://localhost:3000//evil.example/x')))
    # ?settings=<section> on any page opens the dialog over it; an unknown section opens nothing.
    grid['settings_reopen_account'] = settings_case(get('/tracker?settings=account'))
    grid['settings_reopen_unknown'] = settings_case(get('/tracker?settings=nope'))
    grid['settings_otp_enrollment'] = settings_case(two_fa, two_fa)
    grid['settings_otp_enable'] =
      settings_case(two_fa, settings_write('update_two_fa', { '_method' => 'patch', 'user[otp_code_token]' => code_at(AT) }))
    grid['settings_otp_wrong'] =
      settings_case(two_fa, settings_write('update_two_fa', { '_method' => 'patch', 'user[otp_code_token]' => '000000' }, 422))
    grid['settings_otp_no_csrf'] =
      settings_case(two_fa,
                    settings_write('update_two_fa', { '_method' => 'patch', 'user[otp_code_token]' => code_at(AT) }, 302, 'csrf' => 'none'))
    %w[0 1 false true invalid].each do |value|
      grid["settings_wash_#{value}"] =
        settings_case(settings_write('update_wash_sale', { '_method' => 'patch', 'wash_sale[enabled]' => value, 'wash_sale[jurisdiction]' => 'GB' },
                                     200))
    end
    grid['settings_wash_missing'] = settings_case(settings_write('update_wash_sale', { '_method' => 'patch' }, 422))
    grid['settings_wash_invalid_jurisdiction'] =
      settings_case(settings_write('update_wash_sale', { '_method' => 'patch', 'wash_sale[enabled]' => '1', 'wash_sale[jurisdiction]' => 'ZZ' }, 422))
    # Each surface lists its own clients; revoke confirms in place, and answers the section it came from.
    grid['settings_client_list'] = settings_case(get('/settings/api'), fixture: 'clients')
    grid['settings_client_list_mcp'] = settings_case(get('/settings/mcp'), fixture: 'clients')
    grid['settings_client_revoke'] = settings_case(settings_write('revoke_mcp_client/1', { '_method' => 'delete' }), fixture: 'clients')
    grid['settings_client_revoke_rest'] =
      settings_case(settings_write('revoke_mcp_client/1', { '_method' => 'delete', 'surface' => 'rest' }), fixture: 'clients')
    %w[read control trade tax].each do |group|
      grid["settings_client_grant_#{group}"] =
        settings_case(settings_write('update_client_tool_permissions/1', { '_method' => 'patch', 'surface' => 'mcp', 'group' => group, 'enabled' => '1' }), # rubocop:disable Layout/LineLength -- Literal parity request values.
                      fixture: 'clients')
    end
    grid['settings_client_following_list'] = settings_case(get('/settings/mcp'), fixture: 'clients_following')
    [%w[mcp read 0], %w[mcp trade 1], %w[rest read 1], %w[rest read 0]].each do |surface, group, enabled|
      grid["settings_client_following_#{surface}_#{group}_#{enabled}"] =
        settings_case(settings_write('update_client_tool_permissions/1', { '_method' => 'patch', 'surface' => surface, 'group' => group, 'enabled' => enabled }), # rubocop:disable Layout/LineLength -- Literal parity request values.
                      fixture: 'clients_following')
    end
    %w[trading read_only].each do |type|
      request = post('/tracker/add_api_key', { 'exchange_id' => '1', 'key_type' => type,
                                               'api_key[key]' => 'placeholder-key', 'api_key[secret]' => 'placeholder-value-123', 'api_key[passphrase]' => 'paper' }, # rubocop:disable Layout/LineLength -- Literal parity request values.
                     'csrf' => 'header', 'headers' => TURBO, 'expect' => 200, 'settings_snapshot' => true)
      grid["settings_key_save_#{type}"] = settings_case(request, fixture: 'keys')
      grid["settings_key_csrf_#{type}"] = settings_case(request.merge('csrf' => 'none', 'expect' => 302), fixture: 'keys')
    end
    %w[incorrect failure].each do |branch|
      grid["settings_key_#{branch}"] =
        settings_case(
          post('/tracker/add_api_key',
               { 'exchange_id' => '1', 'key_type' => 'trading', 'api_key[key]' => "#{branch}-key", 'api_key[secret]' => 'placeholder-value-123', 'api_key[passphrase]' => 'paper' }, 'csrf' => 'header', 'headers' => TURBO, 'expect' => 422, 'settings_snapshot' => true), fixture: 'keys' # rubocop:disable Layout/LineLength -- Literal parity request values.
        )
    end
    %w[mcp rest].each do |surface|
      %w[read control trade tax].each do |group|
        %w[0 1].each do |enabled|
          grid["settings_global_#{surface}_#{group}_#{enabled}"] =
            settings_case(settings_write("update_#{surface}_tool_group_permissions",
                                         { '_method' => 'patch', 'group' => group, 'enabled' => enabled }))
        end
      end
      grid["settings_global_#{surface}_tool"] =
        settings_case(settings_write("update_#{surface}_tool_permissions", { '_method' => 'patch', 'tool_name' => 'list_bots', 'enabled' => '1' }))
      grid["settings_global_#{surface}_invalid"] =
        settings_case(settings_write("update_#{surface}_tool_permissions", { '_method' => 'patch', 'tool_name' => 'invented', 'enabled' => '1' },
                                     422))
    end
    %w[0 1].each do |on|
      grid["settings_dry_run_#{on}"] = settings_case(settings_write('update_mcp_dry_run', { '_method' => 'patch', 'enabled' => on }))
    end
    {
      'registration_open' => [{ 'registration_open' => 'true' }, owner],
      'advanced' => [{}, owner('advanced_bots_enabled' => true)],
      'smtp_custom' => [
        { 'smtp_provider' => 'custom_smtp', 'smtp_username' => 'placeholder-user', 'smtp_password' => 'placeholder-password',
          'smtp_host' => 'mail.example.com', 'smtp_port' => '2525' }, owner
      ],
      'smtp_dormant' => [
        { 'smtp_username' => 'placeholder-user', 'smtp_password' => 'placeholder-password', 'smtp_host' => 'mail.example.com',
          'smtp_port' => '2525' }, owner
      ],
      'smtp_env_selected' => [{ 'smtp_provider' => 'env_smtp' }, owner]
    }.each do |name, (configs, user)|
      grid["settings_configured_#{name}"] = settings_case(get('/settings/account'), user: user).merge('app_configs' => configs)
    end
    {
      'coingecko' => { 'market_data_provider' => 'coingecko', 'coingecko_api_key' => 'coingecko-key-placeholder' },
      'coingecko_dormant' => { 'coingecko_api_key' => 'coingecko-key-placeholder' },
      'platform_stale' => { 'market_data_provider' => 'deltabadger' }
    }.each do |name, configs|
      grid["settings_connection_state_#{name}"] = settings_case(get('/settings/connect')).merge('app_configs' => configs)
    end
    token = 'abcdefghijklmnopqrst'
    confirmation_user = owner('unconfirmed_email' => 'next@example.com', 'confirmation_token' => token,
                              'confirmation_sent_at' => '1926-01-01T00:00:00Z')
    grid['settings_confirmation_once'] = settings_case(get("/confirmation?confirmation_token=#{token}").merge('settings_snapshot' => true),
                                                       get("/confirmation?confirmation_token=#{token}").merge('settings_snapshot' => true), user: confirmation_user) # rubocop:disable Layout/LineLength -- Literal parity request values.
    grid['settings_confirmation_wrong'] =
      settings_case(get('/confirmation?confirmation_token=wrong-placeholder-token').merge('settings_snapshot' => true), user: confirmation_user)
    grid['settings_confirmation_wrong_other_query'] =
      settings_case(get('/confirmation?confirmation_token=wrong-placeholder-token&marker=keep').merge('settings_snapshot' => true),
                    user: confirmation_user)
    %w[owner@example.com absent@example.com invalid].each_with_index do |address, index|
      request = post('/confirmation', { 'user[email]' => address }, 'csrf' => 'header', 'expect' => 303, 'settings_snapshot' => true)
      grid["settings_confirmation_resend_#{index}"] = settings_case(request, user: confirmation_user)
    end
    grid['settings_confirmation_new'] = settings_case(get('/confirmation/new'))
    I18n.available_locales.each do |locale|
      %w[trading read_only].each do |type|
        path = "/#{locale}/tracker/add_api_key/new?exchange_id=1&key_type=#{type}"
        grid["settings_key_form_#{type}_#{locale}"] = settings_case(get(path), fixture: 'keys')
        grid["settings_key_reconnect_#{type}_#{locale}"] =
          settings_case(get(path, 'Turbo-Frame' => 'modal'), fixture: 'keys')
      end
    end
    grid['settings_key_form_untyped_healthy'] = settings_case(get('/tracker/add_api_key/new?exchange_id=1'), fixture: 'keys')
    I18n.available_locales.each do |locale|
      scenario = settings_case(get("/#{locale}/bots/1/add_api_key/new", 'Turbo-Frame' => 'modal'), fixture: 'keys')
      grid["settings_bot_key_form_#{locale}"] = scenario.merge('install' => 'alpaca', 'api_keys' => {}, 'bots' => [{ 'kind' => 'basket' }])
    end
    %w[placeholder incorrect failure].each do |branch|
      request = post('/bots/1/add_api_key',
                     { 'api_key[key]' => "#{branch}-key", 'api_key[secret]' => 'placeholder-value-123', 'api_key[passphrase]' => 'paper' },
                     'csrf' => 'header', 'headers' => TURBO, 'expect' => branch == 'placeholder' ? 200 : 422, 'settings_snapshot' => true,
                     'action_snapshot' => true, 'may_mutate' => %w[api_keys])
      grid["settings_bot_key_save_#{branch}"] =
        settings_case(request, fixture: 'keys').merge('install' => 'alpaca', 'api_keys' => {}, 'bots' => [{ 'kind' => 'basket' }])
    end
    I18n.available_locales.each do |locale|
      request = post("/#{locale}/tracker/add_api_key",
                     { 'exchange_id' => '1', 'key_type' => 'trading', 'api_key[key]' => 'placeholder-key',
                       'api_key[secret]' => 'placeholder-value-123', 'api_key[passphrase]' => 'paper' },
                     'csrf' => 'header', 'headers' => TURBO, 'expect' => 200, 'settings_snapshot' => true)
      grid["settings_sync_warning_#{locale}"] = settings_case(request, fixture: 'keys_warnings')
    end
    positions_request = post('/tracker/add_api_key',
                             { 'exchange_id' => '1', 'key_type' => 'read_only', 'api_key[key]' => 'positions-bad',
                               'api_key[secret]' => 'placeholder-value-123', 'api_key[passphrase]' => 'paper' },
                             'csrf' => 'header', 'headers' => TURBO, 'expect' => 422, 'settings_snapshot' => true)
    grid['settings_key_positions_incorrect'] = settings_case(positions_request, fixture: 'keys')
    %w[r9-no-cash r9-null-cash r9-unnamed-position].each do |key|
      request = post('/tracker/add_api_key',
                     { 'exchange_id' => '1', 'key_type' => 'read_only', 'api_key[key]' => key,
                       'api_key[secret]' => 'placeholder-value-123', 'api_key[passphrase]' => 'paper' },
                     'csrf' => 'header', 'headers' => TURBO, 'expect' => 422, 'settings_snapshot' => true)
      grid["settings_key_#{key}"] = settings_case(request, fixture: 'keys')
    end
    {
      'unicode_symbol' => ['Correcthorse9Ż', 200],
      'unicode_digit' => ['Correcthorse١!', 422],
      'multiline' => ["Correct\nhorse-9", 422],
      'whitespace' => ['   ', 200]
    }.each do |branch, (value, status)|
      fields = { '_method' => 'patch', 'user[password]' => value, 'user[password_confirmation]' => value, 'user[current_password]' => PASSWORD }
      grid["settings_password_edge_#{branch}"] = settings_case(settings_write('update_password', fields, status))
    end
    alias_request = settings_write('update_email',
                                   { '_method' => 'patch', 'user[email]' => 'alice+tag@googlemail.com', 'user[current_password]' => PASSWORD })
    grid['settings_email_google_alias_taken'] = settings_case(alias_request).merge('extra_users' => [owner('email' => 'alice@gmail.com')])
    {
      'blank_email' => ['update_email', { 'user[email]' => '', 'user[current_password]' => PASSWORD }],
      'custom_email' => ['update_email', { 'user[email]' => 'name@localhost', 'user[current_password]' => PASSWORD }],
      'confirmation_only' => ['update_password', { 'user[password_confirmation]' => 'Another-horse-7', 'user[current_password]' => PASSWORD }],
      'blank_current' => ['update_email', { 'user[email]' => 'next@example.com', 'user[current_password]' => '   ' }],
      'simple_password' => ['update_password', { 'user[password]' => 'simple', 'user[current_password]' => PASSWORD }]
    }.each do |branch, (action, fields)|
      I18n.available_locales.each do |locale|
        request = settings_write(action, { '_method' => 'patch' }.merge(fields), 422)
        request['path'] = "/#{locale}#{request['path']}"
        grid["settings_error_edge_#{branch}_#{locale}"] = settings_case(request)
      end
    end
    grid['settings_email_google_alias_taken_raw'] =
      settings_case(settings_write('update_email',
                                   { '_method' => 'patch', 'user[email]' => ' ALICE+TAG@googlemail.com ',
                                     'user[current_password]' => PASSWORD })).merge('extra_users' => [owner('email' => 'alice@gmail.com')])
    secret_query = get('/confirmation?confirmation_token=wrong&api_key%5Bsecret%5D=previous-secret&marker=keep')
    grid['settings_secret_query_links'] = settings_case(secret_query, fixture: 'keys')
    %w[create revalidate replace].each do |branch|
      kind = branch == 'create' ? 'read_only' : 'trading'
      key = branch == 'revalidate' ? 'previous-key' : 'replacement-key'
      secret = branch == 'revalidate' ? 'previous-secret' : 'replacement-secret'
      request = post('/api/api_keys', { 'api_key[exchange_id]' => '1', 'api_key[key_type]' => kind,
                                        'api_key[key]' => key, 'api_key[secret]' => secret, 'api_key[passphrase]' => 'paper' },
                     'csrf' => 'header', 'expect' => 201, 'settings_snapshot' => true)
      grid["settings_key_legacy_#{branch}"] = settings_case(request, fixture: 'keys').merge('legacy_history' => branch == 'replace')
    end
    [true, false].each do |taken|
      request = settings_write('update_email', { '_method' => 'patch', 'user[email]' => ' TAKEN@example.com ', 'user[current_password]' => PASSWORD })
      resend = post('/confirmation', { 'user[email]' => 'owner@example.com' }, 'csrf' => 'header', 'expect' => 303, 'settings_snapshot' => true)
      confirm = get('/confirmation?confirmation_token=__R2_TOKEN__').merge('confirmation_token_from_user' => true, 'settings_snapshot' => true)
      scenario = settings_case(request, get('/settings/account'), resend, confirm)
      scenario['extra_users'] = [owner('email' => 'taken@example.com')] if taken
      grid["settings_r2_email_resend_confirm_#{taken ? 'taken' : 'available'}"] = scenario
    end
    %w[create invalid].each do |branch|
      valid = branch == 'create'
      fields = { 'api_key' => { 'exchange_id' => valid ? 1 : 999_999, 'key_type' => 'read_only', 'key' => 'json-key', 'secret' => 'json-secret',
                                'passphrase' => 'paper' } }
      request = { 'method' => 'POST', 'path' => '/api/api_keys', 'json' => fields.to_json, 'headers' => { 'Content-Type' => 'application/json' },
                  'csrf' => 'header', 'expect' => valid ? 201 : 422, 'settings_snapshot' => true }
      grid["settings_r2_json_key_#{valid ? 'create' : 'invalid'}"] = settings_case(request, fixture: 'keys')
    end
    grid['settings_key_list'] = settings_case(get('/settings/api_keys'), fixture: 'keys')
    grid['settings_key_list_warnings'] = settings_case(get('/settings/api_keys'), fixture: 'keys_warnings')
    # A key's permissions open in the pane, the sidebar with them; remove confirms in place.
    grid['settings_key_permissions'] = settings_case(get('/settings/api_key_permissions/1', 'Turbo-Frame' => 'settings_pane'), fixture: 'keys')
    grid['settings_key_permissions_page'] = settings_case(get('/settings/api_key_permissions/1'), fixture: 'keys')
    grid['settings_key_delete'] = settings_case(settings_write('destroy_api_key/1', { '_method' => 'delete' }), fixture: 'keys')
    tracker_connect_scenarios(grid)
    market_data_scenarios(grid)
    stocks_platform_scenarios(grid)
    email_and_docs_scenarios(grid)
    ibkr_connect_scenarios(grid)
    venue_key_scenarios(grid)
    preference_scenarios(grid)
    { 'nbsp' => "\u00a0", 'em_space' => "\u2003", 'nul' => "\0" }.each do |name, padding|
      accepted = name == 'nul'
      grid["settings_sign_in_email_#{name}"] = {
        'user' => owner,
        'steps' => [get('/login'), login(email: "#{padding}OWNER@Example.com#{padding}", expect: accepted ? 303 : 422),
                    get('/settings/account').merge('expect' => accepted ? 200 : 302)]
      }
    end
    grid
  end

  # The menu's preferences, the tracker's cash switch and the account page's switches, each followed by the pages that
  # read what was written. A step's 'broadcasts' lists the streams whose cable payloads it records.
  def self.preference_scenarios(grid)
    from = ->(path) { TURBO.merge('Referer' => "http://localhost:3000#{path}") }
    request_id = { 'X-Turbo-Request-Id' => '6b0f2f5e-3c39-4a51-9d55-0d5bfa0c1c7e' }
    pref = ->(action, form, status, opts = {}) { settings_write(action, { '_method' => 'patch' }.merge(form), status, opts) }
    currency = ->(code, status, opts = {}) { pref.call('update_display_currency', { 'user[display_currency]' => code }, status, opts) }
    hide = ->(form, opts = {}) { pref.call('update_hide_balances', form, 303, { 'broadcasts' => ['user_1:preferences'] }.merge(opts)) }
    # Stopped bots with history: a settings step records with ActiveJob's test adapter, so Rails' job table holds no
    # checkpoint for a running bot to count down to. A bots page in another currency than USD needs a rate from the
    # network, so after a currency change the account page is what is read back (the first-sync grid reads the rest).
    stopped_history = [stopped('basket', 'orders' => 'history', 'logs' => 'history'), stopped('single', 'orders' => 'one_fill'), { 'kind' => 'index' }]

    grid['settings_pref_currency_from_bots'] =
      with_bots(stopped_history, signed_in(get('/bots'), currency.call('EUR', 303, 'headers' => from.call('/bots')), get('/settings/account')))
    grid['settings_pref_currency_de'] =
      settings_case(currency.call('EUR', 303, 'path' => '/de/settings/update_display_currency', 'headers' => from.call('/de/settings/account')),
                    get('/de/settings/account'))
    grid['settings_pref_currency_lowercase'] = settings_case(currency.call('gbp', 303))
    grid['settings_pref_currency_blank'] = settings_case(currency.call('', 303), user: owner('display_currency' => 'EUR'))
    grid['settings_pref_currency_same'] = settings_case(currency.call('USD', 303))
    grid['settings_pref_currency_invalid'] = settings_case(currency.call('XYZ', 422))
    grid['settings_pref_currency_invalid_de'] =
      settings_case(currency.call('XYZ', 422, 'path' => '/de/settings/update_display_currency'))
    grid['settings_pref_currency_no_referer'] = settings_case(currency.call('CHF', 303, 'headers' => TURBO.except('Referer')))
    grid['settings_pref_currency_foreign_referer'] = settings_case(currency.call('CHF', 303, 'headers' => TURBO.merge('Referer' => 'http://evil.example/bots')))
    grid['settings_pref_currency_csrf'] = settings_case(currency.call('EUR', 302, 'csrf' => 'none'))

    grid['settings_pref_hide_on'] =
      with_bots(stopped_history, signed_in(get('/bots'), hide.call({ 'hide_balances' => '1' }, 'headers' => from.call('/bots').merge(request_id)),
                                  get('/bots'), get('/bots/1'), get('/bots/1.turbo_stream', FEED), get('/tracker')))
    grid['settings_pref_hide_off'] =
      with_bots(stopped_history, signed_in(get('/bots/1'), hide.call({ 'hide_balances' => '0' }, 'headers' => from.call('/bots/1').merge(request_id)),
                                  get('/bots/1'), get('/bots')), 'user' => owner('hide_balances' => true))
    grid['settings_pref_hide_unchanged'] = settings_case(hide.call({ 'hide_balances' => '1' }), user: owner('hide_balances' => true))
    grid['settings_pref_hide_missing'] = settings_case(hide.call({}, 'headers' => TURBO.except('Referer')), user: owner('hide_balances' => true))
    grid['settings_pref_hide_csrf'] = settings_case(hide.call({ 'hide_balances' => '1' }, 'csrf' => 'none', 'expect' => 302))

    cash = ->(value, opts = {}) { pref.call('update_show_cash', { 'show_cash' => value }, 303, opts) }
    grid['settings_pref_cash_on'] =
      with_bots(three, signed_in(get('/tracker'), cash.call('1', 'headers' => from.call('/tracker')), get('/tracker'), get('/bots')),
                'user' => owner('hide_balances' => true))
    grid['settings_pref_cash_off'] =
      with_bots(three, signed_in(get('/tracker'), cash.call('0', 'headers' => from.call('/tracker')), get('/tracker')),
                'user' => owner('hide_balances' => true, 'tracker_settings' => { 'show_cash' => true }))
    grid['settings_pref_cash_other_keys'] =
      settings_case(cash.call('1', 'headers' => TURBO.except('Referer')),
                    user: owner('tracker_settings' => { 'export_type' => 'a<b>&c', 'show_cash' => false, 'ratio' => 1.5 }))
    grid['settings_pref_cash_null'] = settings_case(cash.call('1'), user: owner('tracker_settings' => nil))
    grid['settings_pref_cash_unchanged'] = settings_case(cash.call('1'), user: owner('tracker_settings' => { 'show_cash' => true }))

    grid['settings_pref_advanced_on'] = settings_case(pref.call('update_advanced_bots', { 'enabled' => '1' }, 200))
    grid['settings_pref_advanced_off'] =
      settings_case(pref.call('update_advanced_bots', { 'enabled' => '0' }, 200), user: owner('advanced_bots_enabled' => true))
    grid['settings_pref_advanced_de'] =
      settings_case(pref.call('update_advanced_bots', { 'enabled' => '1' }, 200, 'path' => '/de/settings/update_advanced_bots'))

    grid['settings_pref_registration_on'] = settings_case(pref.call('update_registration', { 'registration_open' => '1' }, 200))
    grid['settings_pref_registration_off'] =
      settings_case(pref.call('update_registration', { 'registration_open' => '0' }, 200)).merge('app_configs' => { 'registration_open' => 'true' })
    grid['settings_pref_registration_de'] =
      settings_case(pref.call('update_registration', { 'registration_open' => '1' }, 200, 'path' => '/de/settings/update_registration'))
    grid['settings_pref_registration_nonadmin'] =
      settings_case(pref.call('update_registration', { 'registration_open' => '1' }, 403), user: owner('admin' => false))

    custom = { 'smtp_username' => 'placeholder-user', 'smtp_password' => 'placeholder-password', 'smtp_host' => 'mail.example.com',
               'smtp_port' => '2525' }
    email = ->(form, status = 200, opts = {}) { pref.call('update_email_notifications', form, status, opts) }
    grid['settings_pref_email_custom'] = settings_case(email.call({ 'smtp_provider' => 'custom_smtp' }.merge(custom)))
    grid['settings_pref_email_custom_de'] =
      settings_case(email.call({ 'smtp_provider' => 'custom_smtp' }.merge(custom), 200, 'path' => '/de/settings/update_email_notifications'))
    grid['settings_pref_email_custom_no_host'] =
      settings_case(email.call({ 'smtp_provider' => 'custom_smtp', 'smtp_username' => 'placeholder-user', 'smtp_password' => 'placeholder-password' }))
    grid['settings_pref_email_custom_missing'] =
      settings_case(email.call({ 'smtp_provider' => 'custom_smtp', 'smtp_username' => 'placeholder-user', 'smtp_password' => ' ' }))
    grid['settings_pref_email_custom_missing_configured'] =
      settings_case(email.call({ 'smtp_provider' => 'custom_smtp' })).merge('app_configs' => { 'smtp_provider' => 'env_smtp' })
    grid['settings_pref_email_redacted'] =
      settings_case(email.call({ 'smtp_provider' => 'custom_smtp', 'smtp_username' => '[redacted]', 'smtp_password' => '[redacted]',
                                 'smtp_host' => 'mail.example.com', 'smtp_port' => '2525' }))
      .merge('app_configs' => custom)
    grid['settings_pref_email_disable'] =
      settings_case(email.call({ 'smtp_provider' => '' })).merge('app_configs' => { 'smtp_provider' => 'custom_smtp' }.merge(custom))
    grid['settings_pref_email_disable_unset'] = settings_case(email.call({}))
    grid['settings_pref_email_env'] = settings_case(email.call({ 'smtp_provider' => 'env_smtp' }))
    grid['settings_pref_email_unknown'] = settings_case(email.call({ 'smtp_provider' => 'other' }))
    grid['settings_pref_email_nonadmin'] =
      settings_case(email.call({ 'smtp_provider' => 'env_smtp' }, 403), user: owner('admin' => false))

    # A stored attribute the model rejects (here a wash-sale jurisdiction no form offers) fails every save of the user,
    # changed or not: the currency answers render_preference_error, the update! actions raise ActiveRecord::RecordInvalid
    # before anything is written or broadcast.
    invalid = owner('wash_sale_jurisdiction' => 'ZZ')
    raised = { 'expect' => 422, 'rails_exception_status' => 422 }
    # Signed in from the connect page, which neither draws the jurisdiction nor writes anything.
    on_invalid = ->(step, user = invalid) { { 'user' => user, 'steps' => [get('/login'), login, get('/settings/connect'), step] } }
    grid['settings_pref_invalid_stored_currency'] = on_invalid.call(currency.call('EUR', 422))
    grid['settings_pref_invalid_stored_currency_unchanged'] = on_invalid.call(currency.call('USD', 422))
    grid['settings_pref_invalid_stored_currency_both'] = on_invalid.call(currency.call('XYZ', 422))
    grid['settings_pref_invalid_stored_currency_absent'] =
      on_invalid.call(pref.call('update_display_currency', { 'user[ignored]' => '1' }, 422))
    grid['settings_pref_currency_absent'] = settings_case(pref.call('update_display_currency', { 'user[ignored]' => '1' }, 303))
    grid['settings_pref_invalid_stored_hide'] = on_invalid.call(hide.call({ 'hide_balances' => '1' }, raised))
    grid['settings_pref_invalid_stored_hide_unchanged'] = on_invalid.call(hide.call({ 'hide_balances' => '0' }, raised))
    grid['settings_pref_invalid_stored_cash'] = on_invalid.call(cash.call('1', raised))
    grid['settings_pref_invalid_stored_cash_unchanged'] =
      on_invalid.call(cash.call('1', raised), invalid.merge('tracker_settings' => { 'show_cash' => true }))
    grid['settings_pref_invalid_stored_advanced'] =
      on_invalid.call(pref.call('update_advanced_bots', { 'enabled' => '1' }, 422, raised))
    grid['settings_pref_invalid_stored_advanced_unchanged'] =
      on_invalid.call(pref.call('update_advanced_bots', { 'enabled' => '0' }, 422, raised))

    # The personal REST token: rust/tests/pages.rs compares its random values as placeholders, once each side's revealed
    # token is that side's one live token.
    regenerate = lambda do |path = '/settings/regenerate_api_token'|
      post(path, {}, 'csrf' => 'header', 'headers' => TURBO.merge('Turbo-Frame' => 'rest_settings'), 'expect' => 200, 'settings_snapshot' => true)
    end
    grid['settings_pref_regenerate'] = settings_case(get('/settings/api'), regenerate.call)
    grid['settings_pref_regenerate_first'] = settings_case(regenerate.call)
    grid['settings_pref_regenerate_de'] = settings_case(get('/de/settings/api'), regenerate.call('/de/settings/regenerate_api_token'))
  end

  # The key routes on the venues the engine trades (Kraken 3, Binance 4, KuCoin 5 on the venues install), each answered by the
  # key a step submits (pages_wizard.rb's venue stubs): the tracker's form and its connect flow, a correct, a rejected and an
  # inconclusive answer for a trading and a reading key, a replacement, a stored key asked again, the bot's key, the legacy
  # JSON endpoint (a reading, a withdrawal, a replaced and a resubmitted key) and the walkthroughs.
  def self.venue_key_scenarios(grid)
    on = ->(*steps, **attrs) { settings_case(*steps).merge('install' => 'alpaca', 'venues' => true, 'kucoin' => true, 'venue_keys' => {}).merge(attrs.transform_keys(&:to_s)) }
    stored = ->(venue, status = 'correct') { venue == 'kucoin' ? { 'kucoin_key' => status } : { 'venue_keys' => { venue => status } } }
    { 'kraken' => 3, 'binance' => 4, 'kucoin' => 5 }.each do |venue, id|
      pass = venue == 'kucoin' ? { 'api_key[passphrase]' => 'kc-passphrase' } : {}
      save = lambda do |type, key, status, path = '/tracker/add_api_key'|
        post(path, { 'exchange_id' => id.to_s, 'key_type' => type, 'api_key[key]' => key, 'api_key[secret]' => 'c2VjcmV0' }.merge(pass),
             'csrf' => 'header', 'headers' => TURBO, 'expect' => status, 'settings_snapshot' => true, 'session' => %w[tracker_connect])
      end
      form = ->(type, headers = {}, locale = '') { get("#{locale}/tracker/add_api_key/new?exchange_id=#{id}&key_type=#{type}", headers) }
      legacy = lambda do |type, key|
        post('/api/api_keys', { 'api_key[exchange_id]' => id.to_s, 'api_key[key_type]' => type, 'api_key[key]' => key, 'api_key[secret]' => 'c2VjcmV0' }.merge(pass),
             'csrf' => 'header', 'expect' => 201, 'settings_snapshot' => true)
      end
      name = "settings_venue_key_#{venue}"
      grid["#{name}_form"] = on.(form.('read_only'), form.('trading', 'Turbo-Frame' => 'modal'), form.('read_only', {}, '/de'))
      grid["#{name}_read_only_correct"] = on.(form.('read_only'), save.('read_only', "#{venue}-correct", 200))
      grid["#{name}_trading_correct"] = on.(save.('trading', "#{venue}-correct", 200))
      grid["#{name}_rejected"] = on.(save.('trading', "#{venue}-incorrect", 422))
      grid["#{name}_read_only_rejected"] = on.(save.('read_only', "#{venue}-incorrect", 422))
      grid["#{name}_inconclusive"] = on.(save.('trading', "#{venue}-failure", 422))
      grid["#{name}_read_only_inconclusive"] = on.(save.('read_only', "#{venue}-failure", 422, "/de/tracker/add_api_key"))
      grid["#{name}_connect"] =
        on.(get('/tracker/pick_exchange/new'),
            post('/tracker/pick_exchange', { 'exchange_id' => id.to_s }, 'csrf' => 'both', 'headers' => TURBO, 'expect' => 302, 'session' => %w[tracker_connect]),
            get('/tracker/add_api_key/new').merge('session' => %w[tracker_connect]),
            post("/tracker/add_api_key?exchange_id=#{id}&key_type=read_only", { 'api_key[key]' => "#{venue}-correct", 'api_key[secret]' => 'c2VjcmV0' }.merge(pass),
                 'csrf' => 'header', 'headers' => TURBO, 'expect' => 200, 'settings_snapshot' => true, 'session' => %w[tracker_connect]))
      grid["#{name}_replace"] = on.(form.('trading', 'Turbo-Frame' => 'modal'), save.('trading', "#{venue}-correct", 200), **stored.(venue))
      grid["#{name}_passive"] = on.(form.('trading').merge('settings_snapshot' => true), **stored.(venue, 'incorrect'))
      grid["#{name}_legacy_create"] = on.(legacy.('read_only', "#{venue}-correct"), get('/settings/api_keys'), get('/settings/api_key_permissions/1'))
      grid["#{name}_legacy_withdrawal"] = on.(legacy.('withdrawal', "#{venue}-correct"), get('/settings/api_key_permissions/1'))
      grid["#{name}_legacy_replace"] = on.(legacy.('trading', "#{venue}-correct"), **stored.(venue))
      grid["#{name}_legacy_revalidate"] = on.(legacy.('trading', "#{venue}-key"), **stored.(venue, 'incorrect'))
      grid["#{name}_instructions"] = on.(get('/settings/api_keys'), get('/settings/api_key_permissions/1', 'Turbo-Frame' => 'settings_pane'),
                                         get('/settings/api_key_permissions/1'), **stored.(venue))
      next if venue == 'kucoin'

      bot = lambda do |key, status|
        post('/bots/1/add_api_key', { 'api_key[key]' => key, 'api_key[secret]' => 'c2VjcmV0' }, 'csrf' => 'header', 'headers' => TURBO, 'expect' => status,
                                                                                                 'settings_snapshot' => true, 'action_snapshot' => true, 'may_mutate' => %w[api_keys])
      end
      grid["#{name}_bot"] = on.(get('/bots/1/add_api_key/new', 'Turbo-Frame' => 'modal'), bot.("#{venue}-incorrect", 422), bot.("#{venue}-correct", 200),
                                bots: [stopped(venue)])
      grid["#{name}_bot_de"] = on.(get('/de/bots/1/add_api_key/new', 'Turbo-Frame' => 'modal'), bots: [stopped(venue)])
    end
    # Kraken names the permissions a key lacks or must not hold; a working bot key the sync found lacking one is no tracker key.
    permission = lambda do |type, locale = ''|
      post("#{locale}/tracker/add_api_key", { 'exchange_id' => '3', 'key_type' => type, 'api_key[key]' => 'kraken-permission', 'api_key[secret]' => 'c2VjcmV0' },
           'csrf' => 'header', 'headers' => TURBO, 'expect' => 422, 'settings_snapshot' => true)
    end
    grid['settings_venue_key_kraken_permission'] = on.(permission.('trading'), permission.('trading', '/de'), permission.('read_only', '/pl'))
    grid['settings_venue_key_kraken_untyped_healthy'] = on.(get('/tracker/add_api_key/new?exchange_id=3').merge('expect' => 302), **stored.('kraken'))
    grid['settings_venue_key_kraken_untyped_missing_permission'] =
      on.(get('/tracker/add_api_key/new?exchange_id=3'), **stored.('kraken'), key_sync_error: 'EGeneral:Permission denied')
    # Binance: a key without its IP restriction may not trade, and may read.
    unrestricted = lambda do |type, status|
      post('/tracker/add_api_key', { 'exchange_id' => '4', 'key_type' => type, 'api_key[key]' => 'binance-unrestricted', 'api_key[secret]' => 'c2VjcmV0' },
           'csrf' => 'header', 'headers' => TURBO, 'expect' => status, 'settings_snapshot' => true)
    end
    grid['settings_venue_key_binance_unrestricted'] = on.(unrestricted.('trading', 422))
    grid['settings_venue_key_binance_read_only_unrestricted'] = on.(unrestricted.('read_only', 200))
  end

  # Plan IBKR I3: the IBKR connect page (Settings::IbkrConnectionsController) in each of its states, its downloads, the
  # activation's answers, the hosted-only gate with its escape hatch for an existing key, and the disconnect that stops the
  # bots trading on the key. An activation's access token picks IBKR's answer (IBKR_ACCOUNTS).
  def self.ibkr_connect_scenarios(grid)
    hosted = { 'env' => { 'MARKET_DATA_URL' => 'http://data-api.invalid' } }
    page = get('/settings/ibkr_connect')
    on = ->(*steps, key: nil, **attrs) { settings_case(*steps, fixture: 'ibkr').merge(key ? { 'ibkr_key' => key } : {}).merge(attrs.transform_keys(&:to_s)) }
    waiting = { 'status' => 'pending_activation', 'credentials' => true }
    form = lambda do |path, fields, opts = {}|
      post(path, fields, { 'expect' => 302, 'settings_snapshot' => true }.merge(opts))
    end
    grid['settings_ibkr_gated'] = on.(page.merge('expect' => 302))
    grid['settings_ibkr_gated_create'] = on.(form.('/settings/ibkr_connect', {}, 'csrf' => 'header'))
    grid['settings_ibkr_none'] = on.(page, get('/de/settings/ibkr_connect'), **hosted)
    grid['settings_ibkr_create'] = on.(page, post('/settings/ibkr_connect', {}, 'expect' => 302), **hosted)
    # No page offers it over a key (the downloads page's form to this path deletes), so it is sent as a script would.
    grid['settings_ibkr_create_again'] = on.(page, form.('/settings/ibkr_connect', {}, 'csrf' => 'header'), key: {})
    grid['settings_ibkr_downloads'] = on.(page, get('/de/settings/ibkr_connect'), key: {})
    %w[signing encryption dhparam].each do |artifact|
      grid["settings_ibkr_download_#{artifact}"] =
        on.(get("/settings/ibkr_connect/download?artifact=#{artifact}").merge('headers_extra' => %w[content-disposition]), key: {})
    end
    grid['settings_ibkr_download_unknown'] = on.(get('/settings/ibkr_connect/download?artifact=private').merge('expect' => 302), key: {})
    grid['settings_ibkr_download_no_key'] = on.(get('/settings/ibkr_connect/download?artifact=signing').merge('expect' => 302), **hosted)
    grid['settings_ibkr_pending'] = on.(page, key: waiting.merge('updated_at' => '2026-09-08T12:00:00Z'))
    grid['settings_ibkr_stalled'] = on.(page, key: waiting.merge('updated_at' => '2026-08-20T09:30:00Z'))
    grid['settings_ibkr_connected'] = on.(page, key: { 'status' => 'correct', 'credentials' => true })
    # D-IBKR-RECONNECT: a key Rails' create left without its RSA keys (its job not run) offers Disconnect only.
    grid['settings_ibkr_reconnect_generating'] = on.(page, key: { 'generating' => true })
    activate = lambda do |token, secret = IBKR_KEYS['access_token_secret']|
      form.('/settings/ibkr_connect/activate', { 'api_key[key]' => IBKR_KEYS['consumer_key'], 'api_key[access_token]' => token, 'api_key[secret]' => secret })
    end
    grid['settings_ibkr_activate_missing'] = on.(page, activate.('', 'filled'), page, key: {})
    grid['settings_ibkr_activate_pending'] = on.(page, activate.('tok-pending'), page, key: {})
    grid['settings_ibkr_activate_correct'] = on.(page, activate.('tok-correct'), page, key: {})
    grid['settings_ibkr_activate_failure'] = on.(page, activate.(IBKR_FAILING), page, key: {})
    grid['settings_ibkr_activate_resubmit'] = on.(page, activate.('tok-pending'), page, key: waiting.merge('updated_at' => '2026-08-20T09:30:00Z'))
    grid['settings_ibkr_activate_no_key'] = on.(form.('/settings/ibkr_connect/activate', { 'api_key[key]' => 'x' }, 'csrf' => 'header'), **hosted)
    grid['settings_ibkr_destroy'] = on.(page, form.('/settings/ibkr_connect', { '_method' => 'delete' }), page.merge('expect' => 302), key: {})
    grid['settings_ibkr_destroy_hosted'] = on.(page, form.('/settings/ibkr_connect', { '_method' => 'delete' }), page, key: {}, **hosted)
    # The bots on the key's venue stop with it; a bot on another venue keeps working.
    grid['settings_ibkr_destroy_bots'] =
      settings_case(form.('/settings/ibkr_connect', { '_method' => 'delete' }, 'csrf' => 'header', 'may_mutate' => %w[bots]))
      .merge('install' => 'alpaca', 'typed_stocks' => true, 'api_keys' => { 'alpaca' => 'correct', 'ibkr' => 'correct' },
             'bots' => [running('single', '2026-09-08T13:30:00Z', 'exchange' => 'Interactive Brokers'), running('single', '2026-09-08T13:30:00Z')])
  end

  # A17-1: the container's market data on Connect (SettingsController#update_market_data, #disconnect_market_data,
  # #resync_assets) and the tracker's CoinGecko step (TrackerController#connect_market_data, #setup_coingecko, the export modal
  # with no market data). Admin only; a member is refused.
  def self.market_data_scenarios(grid)
    write = ->(action, form, status = 200) { settings_write(action, form, status) }
    patch = ->(form, status = 200) { write.('update_market_data', { '_method' => 'patch' }.merge(form), status) }
    on = ->(*steps, configs: {}, user: owner) { settings_case(get('/settings/connect'), *steps, user:).merge('app_configs' => configs) }
    coingecko = { 'market_data_provider' => 'coingecko', 'coingecko_api_key' => CG_KEY }
    platform = { 'platform_connected_at' => '2026-01-01T00:00:00Z', 'market_data_url' => 'http://data-api.invalid', 'market_data_token' => 'platform-token' }
    member = owner('admin' => false)
    grid['settings_market_data_update_coingecko_new_key'] = on.(patch.({ 'market_data_provider' => 'coingecko', 'coingecko_api_key' => CG_KEY }))
    # D-CG-SETTINGS-KEY-CHECKED: Rails saves a key it never checked.
    grid['settings_market_data_update_coingecko_bad_key'] = on.(patch.({ 'market_data_provider' => 'coingecko', 'coingecko_api_key' => CG_BAD }))
    grid['settings_market_data_update_coingecko_stored_key'] = on.(patch.({ 'market_data_provider' => 'coingecko' }), configs: { 'coingecko_api_key' => CG_KEY })
    grid['settings_market_data_update_coingecko_no_key_form'] = on.(patch.({ 'market_data_provider' => 'coingecko' }))
    grid['settings_market_data_update_off'] = on.(patch.({ 'market_data_provider' => '' }), configs: coingecko)
    grid['settings_market_data_update_deltabadger_refused'] = on.(patch.({ 'market_data_provider' => 'deltabadger' }, 422), configs: coingecko)
    grid['settings_market_data_disconnect'] = on.(write.('disconnect_market_data', { '_method' => 'delete' }), configs: coingecko)
    # D-CG-DISCONNECT-KEEPS-PLATFORM: Rails also drops a claimed platform's data-api credentials.
    grid['settings_market_data_disconnect_platform'] = on.(write.('disconnect_market_data', { '_method' => 'delete' }), configs: coingecko.merge(platform))
    grid['settings_market_data_resync'] = on.(write.('resync_assets', {}), configs: coingecko)
    grid['settings_market_data_update_nonadmin'] =
      settings_case(patch.({ 'market_data_provider' => 'coingecko', 'coingecko_api_key' => CG_KEY }, 403), user: member)
    grid['settings_market_data_disconnect_nonadmin'] =
      settings_case(write.('disconnect_market_data', { '_method' => 'delete' }, 403), user: member).merge('app_configs' => coingecko)
    grid['settings_market_data_resync_nonadmin'] = settings_case(write.('resync_assets', {}, 403), user: member).merge('app_configs' => coingecko)

    connect = ->(path = '/tracker/connect_market_data') { get(path, 'Turbo-Frame' => 'modal') }
    setup = lambda do |key, status = 200|
      post('/tracker/setup_coingecko', { 'api_key' => key }, 'csrf' => 'header', 'headers' => TURBO, 'expect' => status, 'settings_snapshot' => true)
    end
    %w[en de].each { |locale| grid["settings_tracker_connect_market_data_#{locale}"] = settings_case(connect.("/#{locale}/tracker/connect_market_data")) }
    grid['settings_tracker_connect_market_data_nonadmin'] = settings_case(connect.(), user: member)
    grid['settings_tracker_export_modal_unconfigured'] = settings_case(connect.('/tracker/export_modal'))
    grid['settings_setup_coingecko_tracker_ok'] = settings_case(connect.(), setup.(CG_KEY))
    grid['settings_setup_coingecko_tracker_bad_key'] = settings_case(connect.(), setup.(CG_BAD, 422))
    grid['settings_setup_coingecko_tracker_non_admin'] = settings_case(connect.(), setup.(CG_KEY, 403), user: member)
  end

  # A18: SettingsController#update_stocks/#disconnect_stocks (D-STOCKS-RETIRED: Rust refuses them; Rails' answers are the
  # witnesses) and Settings::PlatformConnectionsController (served alike, D-PLATFORM-HOSTED-KEPT aside). Admin only.
  def self.stocks_platform_scenarios(grid)
    write = ->(action, form, status = 200) { settings_write(action, form, status) }
    stocks = ->(form, status = 200) { write.('update_stocks', { '_method' => 'patch' }.merge(form), status) }
    unstock = ->(status = 200) { write.('disconnect_stocks', { '_method' => 'delete' }, status) }
    claim = ->(code, status = 200) { write.('platform_connection', { 'claim_code' => code }, status) }
    unclaim = ->(status = 200) { write.('platform_connection', { '_method' => 'delete' }, status) }
    key = { 'alpaca_api_key' => 'stock-key', 'alpaca_api_secret' => 'stock-secret' }
    stored = { 'alpaca_api_key' => 'stock-key', 'alpaca_api_secret' => 'stock-secret', 'alpaca_mode' => 'paper' }
    platform = { 'platform_connected_at' => '2026-01-01T00:00:00Z', 'market_data_url' => 'http://data-api.invalid',
                 'market_data_token' => 'platform-token', 'market_data_provider' => 'deltabadger', 'proxy_binance' => 'http://proxy.invalid:8080' }
    member = owner('admin' => false)
    # The witnesses' rows before the write (rust/tests/pages.rs a18_divergences: this build's write leaves them as they are).
    snap = get('/settings/account').merge('settings_snapshot' => true)
    grid['settings_stocks_update_new_key'] = settings_case(snap, stocks.(key.merge('alpaca_mode' => 'paper')), fixture: 'venues')
    grid['settings_stocks_update_existing_key'] = settings_case(snap, stocks.(key.merge('alpaca_mode' => 'paper')), fixture: 'keys')
    grid['settings_stocks_update_live'] = settings_case(snap, stocks.(key.merge('alpaca_mode' => 'live')), fixture: 'venues')
    grid['settings_stocks_update_missing'] = settings_case(snap, stocks.({ 'alpaca_api_key' => 'stock-key', 'alpaca_api_secret' => '' }, 422), fixture: 'venues')
    grid['settings_stocks_update_invalid'] = settings_case(snap, stocks.({ 'alpaca_api_key' => 'incorrect-key', 'alpaca_api_secret' => 's' }, 422), fixture: 'venues')
    grid['settings_stocks_update_deltabadger'] = settings_case(stocks.(key, 422), fixture: 'venues').merge('app_configs' => platform)
    grid['settings_stocks_disconnect'] = settings_case(snap, unstock.(), fixture: 'venues').merge('app_configs' => stored)
    grid['settings_stocks_update_nonadmin'] = settings_case(stocks.(key, 403), user: member)
    grid['settings_stocks_disconnect_nonadmin'] = settings_case(unstock.(403), user: member).merge('app_configs' => stored)
    grid['settings_platform_create_proxies'] = settings_case(claim.('dbc_ok')).merge('app_configs' => { 'market_data_provider' => 'coingecko', 'coingecko_api_key' => CG_KEY })
    # Rows that only look like proxies are none (AppConfig.platform_proxies_configured? is a start_with?).
    grid['settings_platform_create_no_proxies'] = settings_case(claim.('dbc_no_proxies')).merge('app_configs' => { 'PROXY_binance' => 'kept', 'proxyx' => 'kept' })
    grid['settings_platform_create_invalid'] = settings_case(claim.('dbc_bad', 422))
    # params[:claim_code]: a query value wins over the body's; a query alone is read too.
    grid['settings_platform_create_query_wins'] = settings_case(write.('platform_connection?claim_code=dbc_no_proxies', { 'claim_code' => 'dbc_bad' }))
    grid['settings_platform_create_query_only'] = settings_case(write.('platform_connection?claim_code=dbc_no_proxies', {}))
    grid['settings_platform_create_reclaim'] = settings_case(claim.('dbc_no_proxies')).merge('app_configs' => platform.merge('market_data_url' => 'http://old-data-api.invalid', 'market_data_token' => 'old-token'))
    grid['settings_platform_destroy'] = settings_case(unclaim.()).merge('app_configs' => platform.merge('PROXY_binance' => 'kept', 'proxyx' => 'kept'))
    grid['settings_platform_destroy_off'] = settings_case(unclaim.()).merge('app_configs' => platform.except('market_data_provider'))
    grid['settings_platform_destroy_coingecko'] = settings_case(unclaim.()).merge('app_configs' => platform.merge('market_data_provider' => 'coingecko', 'coingecko_api_key' => CG_KEY))
    # D-PLATFORM-HOSTED-KEPT: Rails deletes a hosted install's claimed rows, its proxies among them.
    grid['settings_platform_destroy_hosted'] = settings_case(snap, unclaim.()).merge('app_configs' => platform, 'env' => { 'MARKET_DATA_URL' => 'http://env-data-api.invalid', 'MARKET_DATA_TOKEN' => 'env-token' })
    grid['settings_platform_create_nonadmin'] = settings_case(claim.('dbc_ok', 403), user: member)
    grid['settings_platform_destroy_nonadmin'] = settings_case(unclaim.(403), user: member).merge('app_configs' => platform)
  end

  # 3.0 A20: SettingsController#send_test_email and #disconnect_email (admin only), and #download_api_docs (anyone signed in).
  def self.email_and_docs_scenarios(grid)
    test = ->(status = 200, opts = {}) { settings_write('send_test_email', {}, status, opts) }
    disconnect = ->(status = 200) { settings_write('disconnect_email', { '_method' => 'delete' }, status) }
    custom = { 'smtp_provider' => 'custom_smtp', 'smtp_host' => 'smtp.example.com', 'smtp_port' => '587',
               'smtp_username' => 'smtp-user@example.com', 'smtp_password' => 'smtp-secret' }
    member = owner('admin' => false)
    grid['settings_email_test_not_configured'] = settings_case(test.())
    # Saved without credentials is not configured either (SmtpSettings.custom_settings).
    grid['settings_email_test_no_credentials'] = settings_case(test.()).merge('app_configs' => custom.except('smtp_password'))
    grid['settings_email_test_sent'] = settings_case(test.(200, 'mails' => true)).merge('app_configs' => custom)
    # The mail is written in the user's language, whatever the request's.
    grid['settings_email_test_sent_de'] = settings_case(test.(200, 'mails' => true), user: owner('locale' => 'de')).merge('app_configs' => custom)
    grid['settings_email_test_sent_env'] = settings_case(test.(200, 'mails' => true)).merge('env' => { 'SMTP_ADDRESS' => 'smtp.example.com' })
    # D-TEST-EMAIL-ERROR-SAFE: Rails shows the server's reply, which here names the SMTP account.
    grid['settings_email_test_failed'] = settings_case(test.(200, 'mails' => true, 'smtp_failure' => '535 5.7.8 Authentication failed for smtp-user@example.com'))
                                         .merge('app_configs' => custom)
    grid['settings_email_test_nonadmin'] = settings_case(test.(403), user: member).merge('app_configs' => custom)
    grid['settings_email_disconnect'] = settings_case(disconnect.()).merge('app_configs' => custom)
    grid['settings_email_disconnect_env'] = settings_case(disconnect.()).merge('app_configs' => { 'smtp_provider' => 'env_smtp' })
    grid['settings_email_disconnect_nothing'] = settings_case(disconnect.())
    # D-NO-MAIL-SAYS-SO: with registration open, Rails redraws only the mail widget.
    grid['settings_email_disconnect_registration_open'] = settings_case(disconnect.()).merge('app_configs' => custom.merge('registration_open' => 'true'))
    grid['settings_email_disconnect_nonadmin'] = settings_case(disconnect.(403), user: member).merge('app_configs' => custom)
    docs = get('/settings/download_api_docs').merge('headers_extra' => %w[content-disposition])
    grid['settings_api_docs'] = settings_case(docs, get('/de/settings/download_api_docs').merge('headers_extra' => %w[content-disposition]))
    grid['settings_api_docs_member'] = settings_case(docs, user: member)
    grid['settings_api_docs_signed_out'] = { 'user' => owner, 'steps' => [docs] }
  end

  # D5b: the tracker's "connect an exchange" flow (Tracker::PickExchangesController, Tracker::AddApiKeysController).
  def self.tracker_connect_scenarios(grid)
    pick = get('/tracker/pick_exchange/new')
    choose = ->(id, opts = {}) { post('/tracker/pick_exchange', id ? { 'exchange_id' => id } : {}, { 'csrf' => 'both', 'headers' => TURBO }.merge(opts)) }
    I18n.available_locales.each do |locale|
      grid["settings_pick_page_#{locale}"] = settings_case(get("/#{locale}/tracker/pick_exchange/new"), fixture: 'venues')
      grid["settings_pick_frame_#{locale}"] = settings_case(get("/#{locale}/tracker/pick_exchange/new", 'Turbo-Frame' => 'modal'), fixture: 'venues')
    end
    grid['settings_pick_signed_out'] = { 'steps' => [pick] }
    grid['settings_pick_create_alpaca'] = settings_case(pick.merge('session' => %w[tracker_connect]),
                                                        choose.call('1', 'expect' => 302, 'session' => %w[tracker_connect]),
                                                        get('/tracker/add_api_key/new'), fixture: 'venues')
    grid['settings_pick_create_unavailable'] = settings_case(pick, choose.call('7', 'expect' => 302), fixture: 'venues')
    grid['settings_pick_csrf'] = settings_case(pick, choose.call('1', 'csrf' => 'none', 'expect' => 302), fixture: 'venues')
    grid['missing_tracker_pick_unknown'] = settings_case(pick, choose.call('999'), fixture: 'venues')
    grid['missing_tracker_pick_without_id'] = settings_case(pick, choose.call(nil), fixture: 'venues')
    # Interactive Brokers (4): a venue this build does not sync yet.
    grid['not_ported_tracker_key_form'] = settings_case(pick, choose.call('4'), get('/tracker/add_api_key/new'), fixture: 'venues')
    grid['settings_key_form_kraken'] = settings_case(get('/tracker/add_api_key/new?exchange_id=5'), fixture: 'venues')
    grid['settings_key_no_exchange'] = settings_case(get('/tracker/add_api_key/new'), fixture: 'venues')
    grid['settings_key_retired_form'] = settings_case(get('/tracker/add_api_key/new?exchange_id=6&key_type=read_only').merge('expect' => 302),
                                                      fixture: 'venues')
    grid['settings_key_retired_save'] =
      settings_case(post('/tracker/add_api_key', { 'exchange_id' => '6', 'key_type' => 'read_only', 'api_key[key]' => 'placeholder-key',
                                                   'api_key[secret]' => 'placeholder-value-123' },
                         'csrf' => 'header', 'headers' => TURBO, 'expect' => 302, 'settings_snapshot' => true), fixture: 'venues')
    grid['settings_key_untyped_form'] = settings_case(get('/tracker/add_api_key/new?exchange_id=1'), fixture: 'venues')
    grid['settings_key_untyped_reconnect'] = settings_case(get('/tracker/add_api_key/new?exchange_id=1', 'Turbo-Frame' => 'modal'), fixture: 'venues')
    grid['settings_key_untyped_unknown_type'] = settings_case(get('/tracker/add_api_key/new?exchange_id=1&key_type=withdrawal'), fixture: 'keys')
    %w[trading read_only].each do |type|
      grid["settings_key_passive_revalidation_#{type}"] =
        settings_case(get("/tracker/add_api_key/new?exchange_id=1&key_type=#{type}").merge('settings_snapshot' => true), fixture: 'keys')
        .merge('key_status' => 'incorrect')
    end
    grid['settings_key_passive_untyped'] =
      settings_case(get('/tracker/add_api_key/new?exchange_id=1').merge('settings_snapshot' => true), fixture: 'keys').merge('key_status' => 'incorrect')
    grid['settings_key_signed_out_form'] = { 'steps' => [get('/tracker/add_api_key/new?exchange_id=1')] }
    grid['settings_key_signed_out_save'] =
      { 'steps' => [get('/login'), post('/tracker/add_api_key', { 'exchange_id' => '1', 'api_key[key]' => 'placeholder-key' },
                                        'csrf' => 'header', 'headers' => TURBO)] }
  end
end
