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
        ApiKey.create!(user: owner, exchange:, key_type: :trading, status: :correct,
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
      when 'clients'
        other = User.create!(name: 'Other', email: 'other@example.com', password: PASSWORD, confirmed_at: Time.current)
        application = Doorkeeper::Application.create!(name: 'Owner client', uid: 'settings-client', secret: '',
                                                      redirect_uri: 'http://localhost:9911/callback', confidential: false, scopes: 'mcp api')
        [owner, other].each do |user|
          Doorkeeper::AccessToken.create!(application:, resource_owner_id: user.id, token: "settings-token-#{user.id}",
                                          refresh_token: "settings-refresh-#{user.id}", expires_in: 1, scopes: 'mcp')
          ConnectedClient.create!(user:, oauth_application: application, mcp_tools: AppConfig::MCP_TOOL_GROUPS['read'], rest_tools: [])
        end
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
      %w[account connect api].each do |page|
        grid["settings_page_#{page}_#{locale}"] = settings_case(get("/#{locale}/settings/#{page}"))
        grid["settings_frame_#{page}_#{locale}"] = settings_case(get("/#{locale}/settings/#{page}", 'Turbo-Frame' => 'modal'))
      end
    end
    %w[account connect api edit_two_fa].each do |page|
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
        settings_case(settings_write('update_wash_sale', { '_method' => 'patch', 'wash_sale[enabled]' => value, 'wash_sale[jurisdiction]' => 'GB' },
                                     303))
    end
    grid['settings_wash_missing'] = settings_case(settings_write('update_wash_sale', { '_method' => 'patch' }, 422))
    grid['settings_wash_invalid_jurisdiction'] =
      settings_case(settings_write('update_wash_sale', { '_method' => 'patch', 'wash_sale[enabled]' => '1', 'wash_sale[jurisdiction]' => 'ZZ' }, 422))
    grid['settings_client_list'] = settings_case(get('/settings/api'), fixture: 'clients')
    grid['settings_client_revoke_modal'] = settings_case(get('/settings/confirm_revoke_mcp_client/1', 'Turbo-Frame' => 'modal'), fixture: 'clients')
    grid['settings_client_revoke'] = settings_case(settings_write('revoke_mcp_client/1', { '_method' => 'delete' }), fixture: 'clients')
    %w[read control trade tax].each do |group|
      grid["settings_client_grant_#{group}"] =
        settings_case(settings_write('update_client_tool_permissions/1', { '_method' => 'patch', 'surface' => 'mcp', 'group' => group, 'enabled' => '1' }), # rubocop:disable Layout/LineLength -- Literal parity request values.
                      fixture: 'clients')
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
    grid['settings_key_list'] = settings_case(get('/settings/connect'), fixture: 'keys')
    grid['settings_key_permissions'] = settings_case(get('/settings/api_key_permissions/1', 'Turbo-Frame' => 'modal'), fixture: 'keys')
    grid['settings_key_delete_modal'] = settings_case(get('/settings/confirm_destroy_api_key/1', 'Turbo-Frame' => 'modal'), fixture: 'keys')
    grid['settings_key_delete'] = settings_case(settings_write('destroy_api_key/1', { '_method' => 'delete' }), fixture: 'keys')
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
end
