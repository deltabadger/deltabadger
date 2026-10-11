# Records the first-run setup (SetupController and the ApplicationController gate in front of every
# page) for rust/tests/setup.rs, which replays it without Ruby. From a clean export (no .env):
#   s=$(mktemp -d); env -u DATABASE_URL RAILS_ENV=test SKIP_TEST_DATABASE=true APP_ROOT_URL=http://localhost:3000 \
#     PRIMARY_DATABASE_URL=sqlite3:$s/p.sqlite3 QUEUE_DATABASE_URL=sqlite3:$s/q.sqlite3 \
#     CACHE_DATABASE_URL=sqlite3:$s/c.sqlite3 CABLE_DATABASE_URL=sqlite3:$s/w.sqlite3 \
#     sh -c 'bin/rails db:schema:load && bin/rails runner script/rust/setup.rb <root> rust/tests/fixtures/setup_vectors.json'
# <root> is a scratch directory for the per-scenario installs. Re-record when a pinned source (SOURCES)
# changes; rust/tests/setup.rs fails until then.
OAUTH_PARITY = true
load Rails.root.join('script/rust/pages.rb')
require 'digest'

module SetupVectors
  extend ActiveSupport::Testing::TimeHelpers

  module_function

  SOURCES = %w[app/controllers/setup_controller.rb app/controllers/application_controller.rb app/controllers/errors_controller.rb
               app/views/setup/new.html.erb app/views/setup/locked.html.erb app/views/setup/_identity_fields.html.erb
               app/views/setup/_connect_platform.html.erb app/views/layouts/_language_dropdown_full.html.erb
               app/models/user.rb app/models/user/name.rb app/models/user/email.rb app/models/user/password.rb
               config/initializers/devise.rb app/services/platform/redeem_claim.rb app/models/clients/launchpad.rb
               app/jobs/setup/seed_and_sync_job.rb].freeze
  AT = Pages::AT
  TOKEN = 'expected-token'.freeze
  LOCALES = I18n.available_locales.map(&:to_s).sort.freeze
  PASSWORD = Pages::PASSWORD

  def get(path, headers = {}) = { 'method' => 'GET', 'path' => path, 'headers' => headers }
  # 'csrf': 'form' (the hidden field of the last page's form whose action is exactly this path),
  # 'header' (the last page's meta tag in X-CSRF-Token) or 'none'.
  def post(path, form, opts = {}) = { 'method' => 'POST', 'path' => path, 'form' => form, 'csrf' => 'form', 'headers' => {} }.merge(opts)

  def account(locale = 'en', name: 'Owner', email: 'owner@example.com', password: PASSWORD)
    { 'locale' => locale, 'user[name]' => name, 'user[email]' => email, 'user[password]' => password }
  end

  def create(form = account, path: '/setup?locale=en', **opts) = post(path, form, opts.transform_keys(&:to_s))
  def expect(status, step) = step.merge('expect' => status)

  def seed_user(attrs = {})
    { 'name' => 'Member', 'email' => 'member@example.com', 'admin' => false, 'confirmed_at' => '2026-01-01T00:00:00Z' }.merge(attrs)
  end

  def admin = seed_user('name' => 'Owner', 'email' => 'owner@example.com', 'admin' => true, 'setup_completed' => true)

  def page_scenarios
    out = {}
    LOCALES.each do |locale|
      query = locale == 'en' ? '' : "?locale=#{locale}"
      out["new_#{locale}"] = { 'steps' => [expect(200, get("/setup#{query}"))] }
      blank = account(locale, name: '', email: '', password: '')
      out["invalid_#{locale}"] = { 'steps' => [get("/setup#{query}"), expect(422, create(blank, path: "/setup?locale=#{locale}"))] }
      out["locked_#{locale}"] = { 'env' => { 'SETUP_TOKEN' => TOKEN }, 'steps' => [expect(403, get("/setup#{query}"))] }
    end
    out.merge(
      'new_turbo_frame' => { 'steps' => [get('/setup', 'Turbo-Frame' => 'setup_platform_connection')] },
      'new_head' => { 'steps' => [{ 'method' => 'HEAD', 'path' => '/setup', 'headers' => {} }] },
      'new_query' => { 'steps' => [get('/setup?x=1&code=abc&claim_code=def&locale=pl')] },
      'new_connected_without_proxies' => { 'app_configs' => { 'platform_connected_at' => '2026-09-01T00:00:00Z' }, 'steps' => [get('/setup')] },
      'new_connected_with_proxies' => { 'app_configs' => { 'platform_connected_at' => '2026-09-01T00:00:00Z', 'proxy_binance' => 'http://p.example' },
                                        'steps' => [get('/setup')] },
      'invalid_connected' => { 'app_configs' => { 'platform_connected_at' => '2026-09-01T00:00:00Z' },
                               'steps' => [get('/setup'), create(account(name: '', email: 'invalid', password: 'weak'))] }
    )
  end

  def create_scenarios
    long = "Aa1!#{'x' * 125}"
    {
      'create_success' => { 'steps' => [get('/setup'), expect(302, create), expect(302, get('/setup'))] },
      'create_success_de' => { 'steps' => [get('/setup?locale=de'), expect(302, create(account('de'), path: '/setup?locale=de'))] },
      'create_success_ru' => { 'steps' => [get('/setup?locale=ru'), expect(302, create(account('ru'), path: '/setup?locale=ru'))] },
      'create_email_normalized' => { 'steps' => [get('/setup'), expect(302, create(account(email: "  OWNER@Example.COM \t")))] },
      'create_name_newline' => { 'steps' => [get('/setup'), expect(302, create(account(name: "Evil\nOwner")))] },
      'create_name_unicode' => { 'steps' => [get('/setup'), expect(302, create(account(name: 'Zoë  Łukasz')))] },
      'create_password_newline' => { 'steps' => [get('/setup'), expect(302, create(account(password: "Abcdefg1\nxyz")))] },
      'create_password_max' => { 'steps' => [get('/setup'), expect(302, create(account(password: long[0, 128])))] },
      'create_beside_member' => { 'users' => [seed_user], 'steps' => [get('/setup'), expect(302, create)] },
      'invalid_name' => { 'steps' => [get('/setup'), expect(422, create(account(name: 'Owner1')))] },
      'invalid_name_markup' => { 'steps' => [get('/setup'), expect(422, create(account(name: '<b>"x"</b>', email: 'a"b<c>@example.com')))] },
      'invalid_email' => { 'steps' => [get('/setup'), expect(422, create(account(email: 'owner@example')))] },
      'invalid_email_both' => { 'steps' => [get('/setup'), expect(422, create(account(email: 'not an email')))] },
      'invalid_email_taken' => { 'users' => [seed_user('email' => 'owner@example.com')], 'steps' => [get('/setup'), expect(422, create)] },
      'invalid_email_gmail_alias' => { 'users' => [seed_user('email' => 'jan.doe@gmail.com')],
                                       'steps' => [get('/setup'), expect(422, create(account(email: 'jan.doe+setup@gmail.com')))] },
      'invalid_password_short' => { 'steps' => [get('/setup'), expect(422, create(account(password: 'Ab1!')))] },
      'invalid_password_long' => { 'steps' => [get('/setup'), expect(422, create(account(password: long)))] },
      'invalid_password_simple' => { 'steps' => [get('/setup'), expect(422, create(account(password: 'password')))] },
      'invalid_password_blank' => { 'steps' => [get('/setup'), expect(422, create(account(password: '')))] },
      'invalid_all_de' => { 'steps' => [get('/setup?locale=de'),
                                        expect(422, create(account('de', name: '1', email: 'x', password: 'y'), path: '/setup?locale=de'))] },
      'invalid_missing_user' => { 'steps' => [get('/setup'), create({ 'locale' => 'en' })] },
      'csrf_missing' => { 'steps' => [get('/setup'), expect(302, create(account, csrf: 'none')), get('/setup')] },
      'csrf_foreign_origin' => { 'steps' => [get('/setup'), create(account, headers: { 'Origin' => 'http://evil.example' })] },
      'admin_exists' => { 'users' => [admin], 'steps' => [expect(302, get('/setup')), expect(302, get('/setup?locale=de')), get('/login'),
                                                         expect(302, create(account(email: 'other@example.com'), csrf: 'header'))] },
      'create_race' => { 'steps' => [get('/setup').merge('client' => 'a'), get('/setup').merge('client' => 'b'),
                                     expect(302, create.merge('client' => 'b')),
                                     expect(302, create(account(email: 'second@example.com')).merge('client' => 'a'))] }
    }
  end

  def token_scenarios
    locked = { 'SETUP_TOKEN' => TOKEN }
    {
      'token_missing' => { 'env' => locked, 'steps' => [expect(403, get('/setup')), expect(403, get('/setup?locale=de'))] },
      'token_wrong' => { 'env' => locked, 'steps' => [expect(403, get('/setup?token=wrong-token'))] },
      'token_blank_env' => { 'env' => { 'SETUP_TOKEN' => '' },
                             'steps' => [expect(403, get('/setup')), expect(403, get('/setup?token=')),
                                         expect(403, create(account.merge('token' => ''), path: '/setup', csrf: 'header'))] },
      'token_malformed' => { 'env' => locked, 'steps' => [expect(403, get('/setup?token%5B%5D=expected-token')),
                                                          expect(403, get('/setup?token%5Ba%5D=expected-token'))] },
      'token_json' => { 'env' => locked, 'steps' => [expect(403, get('/setup.json'))] },
      'token_post_without' => { 'env' => locked, 'steps' => [get('/setup'), expect(403, create(account, csrf: 'header'))] },
      'token_claim_locked' => { 'env' => locked, 'steps' => [get('/setup'),
                                                             expect(403, post('/setup/platform_connection', { 'claim_code' => 'dbc_manual' },
                                                                              'csrf' => 'header', 'headers' => { 'Accept' => 'text/vnd.turbo-stream.html, text/html, application/xhtml+xml' }))] },
      'token_unlock' => { 'env' => locked, 'steps' => [expect(302, get("/setup?token=#{TOKEN}")), expect(200, get('/setup')),
                                                       expect(302, create), expect(302, get('/setup'))] },
      'token_unlock_de' => { 'env' => locked, 'steps' => [expect(302, get("/setup?locale=de&token=#{TOKEN}")), expect(200, get('/setup?locale=de'))] },
      'token_post_param' => { 'env' => locked, 'steps' => [get('/setup'), expect(302, create(account.merge('token' => TOKEN), path: '/setup', csrf: 'header'))] },
      'token_rotated' => { 'env' => locked, 'steps' => [get("/setup?token=#{TOKEN}"), expect(200, get('/setup')),
                                                        expect(403, get('/setup').merge('env' => { 'SETUP_TOKEN' => 'rotated-token' }))] },
      'token_admin_exists' => { 'env' => locked, 'users' => [admin], 'steps' => [expect(302, get('/setup')), expect(302, get("/setup?token=#{TOKEN}"))] },
      'token_unset_mid_session' => { 'env' => locked, 'steps' => [expect(403, get('/setup')), expect(200, get('/setup').merge('env' => { 'SETUP_TOKEN' => nil }))] }
    }
  end

  # While no admin exists every page of the app zone sends the browser to /setup; what is not
  # ApplicationController (health checks, CSP reports, Doorkeeper's authorize) is untouched.
  def gate_scenarios
    # /settings is a route redirect, answered before any controller; /404, /422 and /500 are static files when the
    # public file server is on (the test environment's), ErrorsController behind the gate when it is off.
    pages = %w[/ /de /login /de/login /bots /bots/1 /settings/account /tracker /confirmation/new /password/new
               /verify_two_factor /bots/1/edit /oauth/authorize /up]
    {
      'gate_pages' => { 'steps' => pages.map { |path| get(path) } },
      'gate_head' => { 'steps' => [{ 'method' => 'HEAD', 'path' => '/bots', 'headers' => {} }] },
      'gate_member_signed_in_post' => { 'users' => [seed_user], 'steps' => [get('/setup'),
                                                                           post('/login', { 'user[email]' => 'member@example.com', 'user[password]' => PASSWORD },
                                                                                'csrf' => 'header')] },
      'gate_csrf_failure' => { 'steps' => [post('/login', { 'user[email]' => 'x@example.com', 'user[password]' => 'x' }, 'csrf' => 'none'), get('/setup')] }
    }
  end

  TURBO = { 'Accept' => 'text/vnd.turbo-stream.html, text/html, application/xhtml+xml', 'Turbo-Frame' => 'setup_platform_connection' }.freeze
  def claim(code = 'dbc_manual', path: '/setup/platform_connection') = post(path, { 'claim_code' => code }, 'headers' => TURBO)

  # The platform claim (W2) against a fake launchpad on 127.0.0.1 (FakeLaunchpad::ANSWERS).
  def claim_scenarios
    out = %w[ok no_proxies no_identity unauthorized incomplete array server_json server_html server_blank not_json close refused].to_h do |answer|
      ["claim_#{answer}", { 'launchpad' => answer, 'steps' => [get('/setup'), claim] }]
    end
    locked = { 'SETUP_TOKEN' => TOKEN, 'CLAIM_TOKEN' => 'dbc_from_docker' }
    out.merge(
      'claim_ok_then_create' => { 'launchpad' => 'ok', 'steps' => [get('/setup'), expect(200, claim), expect(200, get('/setup')),
                                                                   expect(302, create(account(name: 'Owner', email: 'owner@example.com')))] },
      'claim_ok_de' => { 'launchpad' => 'ok', 'steps' => [get('/setup?locale=de'), expect(200, claim(path: '/setup/platform_connection?locale=de')),
                                                          get('/setup?locale=de')] },
      'claim_existing_market_data' => { 'launchpad' => 'ok', 'app_configs' => { 'market_data_url' => 'https://old.example', 'market_data_provider' => 'coingecko' },
                                        'steps' => [get('/setup'), claim] },
      'claim_token_auto' => { 'launchpad' => 'ok', 'env' => { 'CLAIM_TOKEN' => 'dbc_from_docker' }, 'steps' => [expect(200, get('/setup')), get('/setup')] },
      'claim_token_auto_failure' => { 'launchpad' => 'unauthorized', 'env' => { 'CLAIM_TOKEN' => 'dbc_from_docker' }, 'steps' => [expect(200, get('/setup')), get('/setup')] },
      'claim_token_auto_refused' => { 'launchpad' => 'refused', 'env' => { 'CLAIM_TOKEN' => 'dbc_from_docker' }, 'steps' => [get('/setup')] },
      'claim_token_locked' => { 'launchpad' => 'ok', 'env' => locked, 'steps' => [expect(403, get('/setup'))] },
      'claim_token_unlock' => { 'launchpad' => 'ok', 'env' => locked, 'steps' => [get("/setup?token=#{TOKEN}"), expect(200, get('/setup'))] },
      'claim_token_connected' => { 'launchpad' => 'ok', 'env' => { 'CLAIM_TOKEN' => 'dbc_from_docker' },
                                   'app_configs' => { 'platform_connected_at' => '2026-09-01T00:00:00Z' }, 'steps' => [get('/setup')] },
      'claim_locked' => { 'launchpad' => 'ok', 'env' => { 'SETUP_TOKEN' => TOKEN }, 'steps' => [get('/setup'), expect(403, claim.merge('csrf' => 'header'))] },
      'claim_admin_exists' => { 'launchpad' => 'ok', 'users' => [admin], 'steps' => [get('/login'), expect(302, claim.merge('csrf' => 'header'))] }
    )
  end

  def scenarios = page_scenarios.merge(create_scenarios).merge(token_scenarios).merge(gate_scenarios).merge(claim_scenarios)

  # Only 127.0.0.1 is reachable while recording: the fake launchpad. Anything else is refused and written down.
  module LocalOnly
    def initialize(host = nil, *args, **kwargs)
      unless host.to_s == '127.0.0.1'
        Pages::NETWORK << host.to_s
        raise SocketError, "setup vectors: no network (#{host})"
      end
      super
    end
  end

  # The launchpad's POST /api/claim, answered as each scenario needs. 'close' reads the request and hangs up without
  # an answer (the code may have been spent); 'refused' is a port nothing listens on.
  module FakeLaunchpad
    module_function

    IDENTITY = { 'name' => 'Owner', 'email' => 'owner@example.com' }.freeze
    MARKET = { 'url' => 'https://data.example/api', 'token' => 'md-token-123' }.freeze
    JSON_TYPE = 'application/json; charset=utf-8'.freeze
    ANSWERS = {
      'ok' => [200, JSON_TYPE, { 'identity' => IDENTITY, 'market_data' => MARKET,
                                 'proxies' => { 'Binance' => 'http://user:pass@proxy.example:3128', 'kraken' => 'http://proxy2.example:3128' } }.to_json],
      'no_proxies' => [200, JSON_TYPE, { 'identity' => IDENTITY, 'market_data' => MARKET }.to_json],
      'no_identity' => [200, JSON_TYPE, { 'market_data' => MARKET }.to_json],
      'unauthorized' => [401, JSON_TYPE, { 'error' => 'invalid' }.to_json],
      'incomplete' => [200, JSON_TYPE, { 'identity' => IDENTITY, 'market_data' => { 'url' => 'https://data.example/api', 'token' => '' } }.to_json],
      'array' => [200, JSON_TYPE, '[]'],
      'server_json' => [500, JSON_TYPE, { 'error' => 'boom' }.to_json],
      'server_html' => [502, 'text/html', '<html><body>Bad gateway</body></html>'],
      'server_blank' => [503, 'text/plain', ''],
      'not_json' => [200, JSON_TYPE, 'hello']
    }.freeze

    def start(answer, requests)
      server = TCPServer.new('127.0.0.1', 0)
      port = server.addr[1]
      if answer == 'refused'
        server.close
        return [nil, "http://127.0.0.1:#{port}"]
      end
      thread = Thread.new do
        loop do
          client = server.accept
          head = +''
          while (line = client.gets) && line != "\r\n"
            head << line
          end
          body = client.read(head[/^content-length: *(\d+)/i, 1].to_i)
          requests << { 'request_line' => head.lines.first.to_s.strip, 'content_type' => head[/^content-type: *([^\r\n]+)/i, 1], 'body' => body }
          unless answer == 'close'
            status, type, text = ANSWERS.fetch(answer)
            client.write("HTTP/1.1 #{status} Answer\r\nContent-Type: #{type}\r\nContent-Length: #{text.bytesize}\r\nConnection: close\r\n\r\n#{text}")
          end
          client.close
        end
      rescue IOError, Errno::EBADF
        nil
      end
      [-> { server.close; thread.join }, "http://127.0.0.1:#{port}"]
    end
  end

  def with_env(changes)
    kept = changes.keys.to_h { |key| [key, ENV.fetch(key, :absent)] }
    changes.each { |key, value| value.nil? ? ENV.delete(key) : ENV[key] = value }
    yield
  ensure
    kept&.each { |key, value| value == :absent ? ENV.delete(key) : ENV[key] = value }
  end

  def table(name) = ActiveRecord::Base.connection.select_all("SELECT * FROM #{name} ORDER BY id").to_a

  def build(dir, template, scenario)
    FileUtils.mkdir_p(dir)
    %w[production.sqlite3 production_queue.sqlite3].each { |file| FileUtils.cp(File.join(template, file), File.join(dir, file)) }
    Pages.connect(dir)
    travel_to(Time.iso8601('2026-01-01T00:00:00Z')) do
      scenario.fetch('users', []).each do |attrs|
        times = %w[confirmed_at].to_h { |column| [column, attrs[column] && Time.iso8601(attrs[column])] }
        User.new(attrs.merge(times).merge('password' => PASSWORD)).save!(validate: false)
      end
      scenario.fetch('app_configs', {}).each { |key, value| AppConfig.set(key, value) }
    end
    seeded = { 'users' => table('users'), 'app_configs' => table('app_configs') }
    ActiveRecord::Base.connection_pool.disconnect!
    seeded
  end

  def form_token(page, action)
    Pages.forms(page).filter_map { |tag, inner| inner[/name="authenticity_token" value="([^"]+)"/, 1] if tag.include?(%( action="#{CGI.escapeHTML(action)}")) }.first
  end

  def play(dir, scenario)
    Pages.connect(dir)
    Rack::Attack.cache.store = ActiveSupport::Cache::MemoryStore.new
    clients = Hash.new do |hash, name|
      session = ActionDispatch::Integration::Session.new(Rails.application)
      session.host! 'localhost:3000'
      hash[name] = { session:, page: nil, content: {} }
    end
    env = scenario.fetch('env', {}).dup
    env['SETUP_TOKEN'] = nil unless env.key?('SETUP_TOKEN')
    env['CLAIM_TOKEN'] = nil unless env.key?('CLAIM_TOKEN')
    requests = []
    stop = nil
    if scenario['launchpad']
      stop, url = FakeLaunchpad.start(scenario['launchpad'], requests)
      env['LAUNCHPAD_URL'] = url
    end
    ActiveJob::Base.queue_adapter = :test
    ActiveJob::Base.queue_adapter.enqueued_jobs.clear
    Pages::NETWORK.clear
    responses = scenario['steps'].each_with_index.map do |step, index|
      env.merge!(step['env']) if step['env']
      client = clients[step['client'] || 'main']
      travel_to(Time.iso8601(AT), with_usec: true) do
        with_env(env) do
          headers = step['headers'].dup
          params = step['form']&.dup
          if step['csrf'] == 'form'
            params['authenticity_token'] = form_token(client[:page], step['path']) or raise "#{dir} step #{index}: no form posting to #{step['path']}"
          end
          headers['X-CSRF-Token'] = Pages.meta_token(client[:page]) if step['csrf'] == 'header'
          client[:session].process(step['method'].downcase.to_sym, step['path'], params:, headers:)
        end
      end
      response = client[:session].response
      Pages.verify_rendered!("#{dir} step #{index}", client[:session])
      client[:page] = response.body if Pages.meta_token(response.body)
      content = client[:session].request.session.to_h.except('session_id')
      changed = content != client[:content]
      client[:content] = content
      raise "#{dir} step #{index}: expected #{step['expect']}, Rails answered #{response.status}" if step['expect'] && step['expect'] != response.status

      { 'status' => response.status, 'headers' => Pages::HEADERS.to_h { |name| [name, response.headers[name]] }.compact,
        'body' => response.body, 'session_changed' => changed, 'session_keys' => content.keys.sort }
    end
    travel_back
    stop&.call
    raise "#{dir}: Rails reached the network: #{Pages::NETWORK}" unless Pages::NETWORK.empty?

    result = { 'responses' => responses, 'users' => table('users'), 'app_configs' => table('app_configs'),
               'launchpad_url' => env['LAUNCHPAD_URL'], 'launchpad_requests' => requests, 'enqueued' => ActiveJob::Base.queue_adapter.enqueued_jobs.map { |job| job[:job].name } }
    ActiveRecord::Base.connection_pool.disconnect!
    result
  end

  # User#valid? for the setup form's three fields, in every locale: full_messages in Rails' order.
  INPUTS = [
    ['', '', ''], ['Owner', 'owner@example.com', PASSWORD], ['Owner1', 'owner@example.com', PASSWORD], ["Evil\nOwner", 'owner@example.com', PASSWORD],
    ["1\nOwner", 'owner@example.com', PASSWORD], ['Ann  Lee', 'owner@example.com', PASSWORD], ["Ann Lee", 'owner@example.com', PASSWORD],
    ["Ann\tLee", 'owner@example.com', PASSWORD], [' Ann', 'owner@example.com', PASSWORD], ['Ann ', 'owner@example.com', PASSWORD],
    ['Owner', 'owner@example', PASSWORD], ['Owner', 'not an email', PASSWORD], ['Owner', 'a@b@example.com', PASSWORD],
    ['Owner', "x\nowner@example.com", PASSWORD], ['Owner', "owner@example.com\nx", PASSWORD], ['Owner', '  OWNER@Example.COM  ', PASSWORD],
    ['Owner', " owner@example.com", PASSWORD], ['Owner', 'member@example.com', PASSWORD], ['Owner', 'MEMBER@example.com', PASSWORD],
    ['Owner', 'jan.doe+x@gmail.com', PASSWORD], ['Owner', 'Jan.Doe@googlemail.com', PASSWORD], ['Owner', 'jandoe@gmail.com', PASSWORD], ['Owner', 'jan.doe@gmail.com', PASSWORD], ['Owner', 'JAN.DOE+a@GMAIL.COM', PASSWORD],
    ['Owner', 'jan.doe@gmail.com.evil', PASSWORD], ['', '', 'Ab1!'],
    ['Owner', 'owner@example.com', 'Ab1!'], ['Owner', 'owner@example.com', 'Ab1!xyz'], ['Owner', 'owner@example.com', 'Ab1!xyzw'],
    ['Owner', 'owner@example.com', "Aa1!#{'x' * 124}"], ['Owner', 'owner@example.com', "Aa1!#{'x' * 125}"], ['Owner', 'owner@example.com', 'password'],
    ['Owner', 'owner@example.com', 'PASSWORD1!'], ['Owner', 'owner@example.com', 'Password1'], ['Owner', 'owner@example.com', 'Password!'],
    ['Owner', 'owner@example.com', 'Password_1'], ['Owner', 'owner@example.com', "Abcdefg1\nxyz"], ['Owner', 'owner@example.com', "abc\nAbcdefg1!"],
    ['Owner', 'owner@example.com', "Abcdef1!\n"], ['Owner', 'owner@example.com', "Abcdefg1\n"], ['Owner', 'owner@example.com', "\nAbcdefg1"],
    ['Owner', 'owner@example.com', 'Ünïcödé1!x'], ['Owner', 'owner@example.com', 'Abcdefg１!'], ['Owner', 'owner@example.com', '        '],
    ['', 'x', 'y'], ['1', '', 'Ab1!'], ['', 'member@example.com', '']
  ].freeze

  def validations
    out = []
    ActiveRecord::Base.transaction do
      User.new(name: 'Member', email: 'member@example.com', password: PASSWORD, confirmed_at: Time.current).save!(validate: false)
      User.new(name: 'Gmail', email: 'jan.doe@gmail.com', password: PASSWORD, confirmed_at: Time.current).save!(validate: false)
      LOCALES.each do |locale|
        INPUTS.each do |name, email, password|
          I18n.with_locale(locale) do
            user = User.new(name:, email:, password:, admin: true, confirmed_at: Time.current, setup_completed: true, locale:)
            valid = user.valid? # normalises the email first
            out << { 'locale' => locale, 'name' => name, 'email' => email, 'password' => password,
                     'normalized_email' => user.email, 'valid' => valid, 'messages' => user.errors.full_messages,
                     'password_messages' => user.errors[:password] }
          end
        end
      end
      raise ActiveRecord::Rollback
    end
    out
  end

  def run(root, output)
    ActionController::Base.allow_forgery_protection = true
    Rack::Attack.enabled = true
    validated = validations
    TCPSocket.prepend(LocalOnly)
    template = Pages.template(root)
    recorded = scenarios.to_h do |name, scenario|
      dir = File.join(root, name)
      seeded = build(dir, template, scenario)
      [name, scenario.slice('env', 'launchpad').merge('seed' => seeded, 'steps' => scenario['steps']).merge(play(dir, scenario))]
    end
    vector = {
      'sources' => SOURCES.to_h { |path| [path, Digest::SHA256.file(Rails.root.join(path)).hexdigest] },
      'at' => AT, 'secret_key_base' => Rails.application.secret_key_base,
      'patterns' => { 'name' => User::Name::PATTERN, 'email' => User::Email::ADDRESS_PATTERN, 'password' => User::Password::PATTERN,
                      'devise_email' => Devise.email_regexp.source, 'password_length' => [Devise.password_length.min, Devise.password_length.max] },
      'validations' => validated,
      'launchpad_answers' => FakeLaunchpad::ANSWERS.transform_values { |status, type, body| { 'status' => status, 'content_type' => type, 'body' => body } },
      'scenarios' => recorded
    }
    File.write(output, "#{JSON.pretty_generate(vector)}\n")
    puts "#{recorded.size} scenarios, #{vector['validations'].size} validation cases -> #{output}"
  end
end

root, output = ARGV
raise ArgumentError, 'usage: setup.rb <scratch root> <output.json>' unless root && output

SetupVectors.run(root, output)
