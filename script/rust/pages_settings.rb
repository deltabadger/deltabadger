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
        %w[mcp_settings rest_settings tracker_settings settings transient_data mcp_tools rest_tools].each do |column|
          row[column] = JSON.parse(row[column]) if row[column].is_a?(String)
        end
      end
      [table, rows]
    end
  end

  module SettingsGrid
    def scenarios
      super.merge(settings_scenarios)
    end

    def action_fixture(scenario)
      super
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
      when 'venues'
        # D5b: every venue the tracker's picker may list. Gemini is not available; BitMart is retired but still available.
        [['Alpaca', 'Alpaca'], ['Binance', 'Binance'], ['BinanceUs', 'Binance.US'], ['Coinbase', 'Coinbase'], ['Kraken', 'Kraken'],
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
      # Validations call the real client and model against a deterministic paper account response.
      WebMock.stub_request(:get, 'https://paper-api.alpaca.markets/v2/positions').to_return(status: 200, body: '[]',
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
      super
    ensure
      WebMock.reset!
      WebMock.disable!
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
    grid['settings_otp_enrollment'] = settings_case(get('/settings/edit_two_fa'), get('/settings/edit_two_fa'))
    grid['settings_otp_enable'] =
      settings_case(get('/settings/edit_two_fa'), settings_write('update_two_fa', { '_method' => 'patch', 'user[otp_code_token]' => code_at(AT) }))
    grid['settings_otp_wrong'] =
      settings_case(get('/settings/edit_two_fa'), settings_write('update_two_fa', { '_method' => 'patch', 'user[otp_code_token]' => '000000' }, 422))
    grid['settings_otp_no_csrf'] =
      settings_case(get('/settings/edit_two_fa'),
                    settings_write('update_two_fa', { '_method' => 'patch', 'user[otp_code_token]' => code_at(AT) }, 302, 'csrf' => 'none'))
    %w[0 1 false true invalid].each do |value|
      grid["settings_wash_#{value}"] =
        settings_case(settings_write('update_wash_sale', { '_method' => 'patch', 'wash_sale[enabled]' => value, 'wash_sale[jurisdiction]' => 'GB' }))
    end
    grid['settings_wash_missing'] = settings_case(settings_write('update_wash_sale', { '_method' => 'patch' }, 422))
    grid['settings_wash_invalid_jurisdiction'] =
      settings_case(settings_write('update_wash_sale', { '_method' => 'patch', 'wash_sale[enabled]' => '1', 'wash_sale[jurisdiction]' => 'ZZ' }, 422))
    grid['settings_client_list'] = settings_case(get('/settings/api'), fixture: 'clients')
    # The confirm page is gone (the dialog asks in place); its old URL is no route.
    grid['missing_settings_client_revoke_modal'] = settings_case(get('/settings/confirm_revoke_mcp_client/1', 'Turbo-Frame' => 'modal'), fixture: 'clients')
    grid['settings_client_revoke'] = settings_case(settings_write('revoke_mcp_client/1', { '_method' => 'delete' }), fixture: 'clients')
    %w[read control trade tax].each do |group|
      grid["settings_client_grant_#{group}"] =
        settings_case(settings_write('update_client_tool_permissions/1', { '_method' => 'patch', 'surface' => 'mcp', 'group' => group, 'enabled' => '1' }), # rubocop:disable Layout/LineLength -- Literal parity request values.
                      fixture: 'clients')
    end
    grid['settings_client_following_list'] = settings_case(get('/settings/api'), fixture: 'clients_following')
    grid['settings_client_list_mcp'] = settings_case(get('/settings/mcp'), fixture: 'clients')
    grid['settings_client_following_list_mcp'] = settings_case(get('/settings/mcp'), fixture: 'clients_following')
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
      'stocks_paper' => { 'alpaca_api_key' => 'catalog-key-placeholder', 'alpaca_api_secret' => 'catalog-secret-placeholder',
                          'alpaca_mode' => 'paper' },
      'stocks_live' => { 'alpaca_api_key' => 'catalog-key-placeholder', 'alpaca_api_secret' => 'catalog-secret-placeholder',
                         'alpaca_mode' => 'live' },
      'coingecko' => { 'market_data_provider' => 'coingecko', 'coingecko_api_key' => 'coingecko-key-placeholder' },
      'coingecko_dormant' => { 'coingecko_api_key' => 'coingecko-key-placeholder' },
      'platform_stale' => { 'market_data_provider' => 'deltabadger' }
    }.each do |name, configs|
      grid["settings_connection_state_#{name}"] = settings_case(get('/settings/connect')).merge('app_configs' => configs)
    end
    grid['settings_connection_state_catalog'] = settings_case(get('/settings/connect')).merge('install' => 'alpaca', 'api_keys' => {})
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
                     'csrf' => 'header', 'headers' => TURBO, 'expect' => branch == 'placeholder' ? 200 : 422, 'settings_snapshot' => true)
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
    grid['settings_key_permissions'] = settings_case(get('/settings/api_key_permissions/1', 'Turbo-Frame' => 'modal'), fixture: 'keys')
    grid['missing_settings_key_delete_modal'] = settings_case(get('/settings/confirm_destroy_api_key/1', 'Turbo-Frame' => 'modal'), fixture: 'keys')
    grid['settings_key_delete'] = settings_case(settings_write('destroy_api_key/1', { '_method' => 'delete' }), fixture: 'keys')
    tracker_connect_scenarios(grid)
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

  # D5b: the tracker's "connect an exchange" flow (Tracker::PickExchangesController, Tracker::AddApiKeysController).
  def self.tracker_connect_scenarios(grid)
    pick = get('/tracker/pick_exchange/new')
    choose = ->(id, opts = {}) { post('/tracker/pick_exchange', id ? { 'exchange_id' => id } : {}, { 'csrf' => 'both', 'headers' => TURBO }.merge(opts)) }
    I18n.available_locales.each do |locale|
      grid["settings_pick_page_#{locale}"] = settings_case(get("/#{locale}/tracker/pick_exchange/new"), fixture: 'venues')
      grid["settings_pick_frame_#{locale}"] = settings_case(get("/#{locale}/tracker/pick_exchange/new", 'Turbo-Frame' => 'modal'), fixture: 'venues')
    end
    grid['settings_pick_signed_out'] = { 'steps' => [pick] }
    grid['settings_pick_create_alpaca'] = settings_case(pick, choose.call('1', 'expect' => 302), get('/tracker/add_api_key/new'), fixture: 'venues')
    grid['settings_pick_create_unavailable'] = settings_case(pick, choose.call('7', 'expect' => 302), fixture: 'venues')
    grid['settings_pick_csrf'] = settings_case(pick, choose.call('1', 'csrf' => 'none', 'expect' => 302), fixture: 'venues')
    grid['missing_tracker_pick_unknown'] = settings_case(pick, choose.call('999'), fixture: 'venues')
    grid['missing_tracker_pick_without_id'] = settings_case(pick, choose.call(nil), fixture: 'venues')
    grid['not_ported_tracker_key_form_unavailable'] = settings_case(pick, choose.call('7'), get('/tracker/add_api_key/new'), fixture: 'venues')
    grid['not_ported_tracker_key_form_kraken'] = settings_case(get('/tracker/add_api_key/new?exchange_id=5'), fixture: 'venues')
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
