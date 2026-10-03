# The Rails half of the page-parity harness (rust/tests/pages.rs).
#   bin/rails runner script/rust/pages.rb grid <root>    # one install per scenario, with scenario.json
#   bin/rails runner script/rust/pages.rb record <root>  # Rails' answers per <root>/<scenario>/ -> rails.json
# Run it as rust/tests/pages.rs does: RAILS_ENV=test, with every *_DATABASE_URL pointing at scratch files
# that have the schemas loaded (bin/rails db:schema:load). Test, not development: development adds a
# profiler script and cookie to every page and serves assets without fingerprints.
# PAGES=login,two_factor limits the grid to scenarios whose name starts with one of the prefixes.
require 'json'
require 'fileutils'
require 'active_support/testing/time_helpers'

module Pages
  extend ActiveSupport::Testing::TimeHelpers

  module_function

  PASSWORD = 'Correct-horse-9'.freeze
  OTP_SEED = 'JBSWY3DPEHPK3PXP'.freeze
  # Mid-minute, so a rate-limit window does not roll over during a scenario; with microseconds, so a
  # timestamp written without them shows.
  AT = '2026-09-10T12:00:30.123456Z'.freeze
  USER_COLUMNS = %w[id failed_attempts locked_at last_otp_at remember_created_at updated_at].freeze
  HEADERS = %w[location content-type cache-control x-frame-options x-xss-protection x-content-type-options
               x-permitted-cross-domain-policies referrer-policy content-security-policy-report-only retry-after set-cookie].freeze

  def user(attrs = {})
    { 'name' => 'Owner', 'email' => 'owner@example.com', 'admin' => true, 'setup_completed' => true,
      'confirmed_at' => '2026-01-01T00:00:00Z' }.merge(attrs)
  end

  def two_factor_user(attrs = {}) = user({ 'otp_module' => 1, 'otp_secret_key' => OTP_SEED }.merge(attrs))

  def get(path, headers = {}) = { 'method' => 'GET', 'path' => path, 'headers' => headers }

  # 'csrf' says where the step carries a CSRF token, as a real browser would:
  #   'form'   the hidden authenticity_token of the form the last page has for this path (the default);
  #   'header' the last page's csrf-token meta tag, in X-CSRF-Token, as the compiled JS's fetch calls send it;
  #   'both'   a Turbo form submission: the form's field and the header;
  #   'none'   nowhere.
  # 'expect' on a step is the status Rails itself must answer, so that two equal wrong answers cannot pass.
  def post(path, form, opts = {}) = { 'method' => 'POST', 'path' => path, 'form' => form, 'csrf' => 'form', 'headers' => {} }.merge(opts)

  def login(password = PASSWORD, path: '/login', email: 'owner@example.com', **opts)
    post(path, { 'user[email]' => email, 'user[password]' => password, 'user[remember_me]' => '0' }, opts.transform_keys(&:to_s))
  end

  def otp(code, path: '/verify_two_factor') = post(path, { 'user[otp_code_token]' => code })
  def logout(path = '/logout') = post(path, { '_method' => 'delete' })
  def code_at(time) = ROTP::TOTP.new(OTP_SEED).at(Time.iso8601(time))

  # What Turbo sends with a form submission, besides the form's own field and the X-CSRF-Token header.
  TURBO = { 'Accept' => 'text/vnd.turbo-stream.html, text/html, application/xhtml+xml', 'Origin' => 'http://localhost:3000',
            'Referer' => 'http://localhost:3000/login' }.freeze

  def scenarios
    {
      'login_page_en' => { 'steps' => [get('/login')] },
      'login_page_en_prefixed' => { 'steps' => [get('/en/login')] },
      'login_page_de' => { 'steps' => [get('/de/login')] },
      'login_page_ru' => { 'steps' => [get('/ru/login')] },
      'login_page_unknown_locale_param' => { 'steps' => [get('/login?locale=zz')] },
      'login_page_query' => { 'steps' => [get('/login?x=1&host=evil.example&locale=de&b=two+words&a%5B%5D=1')] },
      'login_page_repeated_query_keys' => { 'steps' => [get('/login?locale=de&x=1&user%5Bemail%5D=first%40example.com&locale=pl&x=2' \
                                                            '&user%5Bemail%5D=last%40example.com&a%5B%5D=2&z=0&a%5B%5D=10&a%5B%5D=1')] },
      'login_page_trailing_slash' => { 'steps' => [get('/login/'), get('//de//login/')] },
      'login_page_registration_open' => { 'app_configs' => { 'registration_open' => 'true' }, 'steps' => [get('/login')] },
      'login_page_turbo_frame' => { 'steps' => [get('/login', 'Turbo-Frame' => 'modal')] },
      'head_request' => { 'steps' => [{ 'method' => 'HEAD', 'path' => '/login', 'headers' => {} }] },
      'csrf_failure' => { 'steps' => [get('/login'), login(csrf: 'none', headers: { 'Referer' => 'http://localhost:3000/login?x=1' }),
                                      get('/login')] },
      'csrf_failure_no_referer' => { 'steps' => [get('/login'), login(csrf: 'none')] },
      'csrf_failure_foreign_referer' => { 'steps' => [get('/login'), login(csrf: 'none', headers: { 'Referer' => 'http://evil.example/x' })] },
      'csrf_failure_de' => { 'steps' => [get('/de/login'), login(path: '/de/login', csrf: 'none'), get('/de/login')] },
      'csrf_foreign_origin' => { 'steps' => [get('/login'), login(headers: { 'Origin' => 'http://evil.example' }), get('/login')] },
      'csrf_failure_double_slash_referer' => { 'steps' => [get('/login'),
                                                           login(csrf: 'none', headers: { 'Referer' => 'http://localhost:3000//evil.test/path' })] },
      'stricter_referer_other_scheme' => { 'steps' => [get('/login'),
                                                       login(csrf: 'none', headers: { 'Referer' => 'https://localhost:3000//evil.test/path' })] },
      'csrf_origin_other_scheme' => { 'steps' => [get('/login'), login(headers: { 'Origin' => 'https://localhost:3000' }), get('/login')] },
      'login_wrong_password' => { 'steps' => [get('/login'), login('wrong')] },
      'login_wrong_password_de' => { 'steps' => [get('/de/login'), login('wrong', path: '/de/login')] },
      'login_unknown_email' => { 'steps' => [get('/login'), login(email: 'nobody@example.com')] },
      'login_blank' => { 'steps' => [get('/login'), post('/login', {})] },
      'login_prefilled_email' => { 'steps' => [get('/login'), login('wrong', email: 'a"b<c>@example.com')] },
      'login_email_normalised' => { 'steps' => [get('/login'), login(email: '  OWNER@Example.com ')] },
      'login_long_password' => { 'steps' => [get('/login'), login(PASSWORD + ('x' * 200))] },
      'csrf_header_token' => { 'steps' => [get('/login'), login('wrong', csrf: 'header')] },
      'csrf_invalid_form_valid_header' => { 'steps' => [get('/login'),
                                                        post('/login', { 'user[email]' => 'owner@example.com', 'user[password]' => 'wrong',
                                                                         'authenticity_token' => 'not-a-token' }, 'csrf' => 'header')] },
      'login_turbo_submit' => { 'steps' => [get('/login'), login('wrong', csrf: 'both', headers: TURBO)] },
      'login_turbo_submit_success' => { 'steps' => [get('/login'), login(csrf: 'both', headers: TURBO).merge('expect' => 303)] },
      'csrf_same_origin' => { 'steps' => [get('/login'), login('wrong', headers: { 'Origin' => 'http://localhost:3000' })] },
      'login_fifth_failure_locks' => { 'user' => user('failed_attempts' => 4), 'steps' => [get('/login'), login('wrong')] },
      'login_locked_correct_password' => { 'user' => user('failed_attempts' => 5, 'locked_at' => '2026-09-10T11:59:30Z'),
                                           'steps' => [get('/login'), login] },
      'login_lock_expired' => { 'user' => user('failed_attempts' => 5, 'locked_at' => '2026-09-10T11:45:29Z'),
                                'steps' => [get('/login'), login, get('/')] },
      'login_lock_not_yet_expired' => { 'user' => user('failed_attempts' => 5, 'locked_at' => '2026-09-10T11:45:30.123456Z'),
                                        'steps' => [get('/login'), login] },
      'login_unconfirmed' => { 'user' => user('confirmed_at' => nil), 'steps' => [get('/login'), login, get('/login')] },
      'login_unconfirmed_de' => { 'user' => user('confirmed_at' => nil),
                                  'steps' => [get('/de/login'), login(path: '/de/login'), get('/de/login')] },
      'login_unconfirmed_counter' => { 'user' => user('confirmed_at' => nil, 'failed_attempts' => 2), 'steps' => [get('/login'), login] },
      'login_success' => { 'user' => user('failed_attempts' => 2),
                           'steps' => [get('/login'), login.merge('expect' => 303), get('/').merge('expect' => 302), get('/login')] },
      'login_success_return_to' => { 'steps' => [get('/bots?filter=active'), get('/login'), login] },
      'login_success_user_locale' => { 'user' => user('locale' => 'de'), 'steps' => [get('/login'), login, get('/?locale=de')] },
      'login_success_explicit_locale' => { 'user' => user('locale' => 'de'), 'steps' => [get('/pl/login'), login(path: '/pl/login')] },
      'logout_signed_out' => { 'steps' => [get('/login'), logout.merge('csrf' => 'none'), get('/'), get('/login')] },
      'logout_signed_out_de' => { 'steps' => [get('/de/login'), logout('/de/logout').merge('csrf' => 'none'), get('/de/login')] },
      'root_signed_out' => { 'steps' => [get('/'), get('/de'), get('/en'), get('/?locale=de')] },
      'root_signed_in' => { 'user' => user('locale' => 'pl'), 'steps' => [get('/login'), login, get('/'), get('/en'), get('/de')] },
      'bots_requires_sign_in' => { 'steps' => [get('/bots'), get('/login')] },
      'bots_requires_sign_in_de' => { 'steps' => [get('/de/bots'), get('/de/login'), login(path: '/de/login')] },
      'bots_requires_sign_in_param' => { 'steps' => [get('/bots?locale=de'), get('/login'), get('/en/bots'), get('/en/login')] },
      'locked_while_signed_in' => { 'steps' => [get('/login'), login, get('/')] +
        ([get('/login')] + Array.new(5) { login('wrong') }).map { |step| step.merge('client' => 'other') } + [get('/bots'), get('/login')] },
      'password_changed_while_signed_in' => { 'steps' => [get('/login'), login, get('/bots').merge('before' => 'change_password'),
                                                          get('/login')] },
      'two_factor_page' => { 'user' => two_factor_user, 'steps' => [get('/login'), login, get('/verify_two_factor')] },
      'two_factor_page_user_locale' => { 'user' => two_factor_user('locale' => 'de'),
                                         'steps' => [get('/login'), login, get('/de/verify_two_factor')] },
      'two_factor_explicit_locale' => { 'user' => two_factor_user('locale' => 'de'),
                                        'steps' => [get('/pl/login'), login(path: '/pl/login'), get('/pl/verify_two_factor'),
                                                    otp(code_at(AT), path: '/pl/verify_two_factor')] },
      'two_factor_wrong_code' => { 'user' => two_factor_user, 'steps' => [get('/login'), login, get('/verify_two_factor'), otp('000000')] },
      'two_factor_blank_code' => { 'user' => two_factor_user, 'steps' => [get('/login'), login, get('/verify_two_factor'), otp('')] },
      'two_factor_success' => { 'user' => two_factor_user('failed_attempts' => 3),
                                'steps' => [get('/login'), login, get('/verify_two_factor'), otp(code_at(AT)).merge('expect' => 303), get('/')] },
      'two_factor_previous_step_accepted' => { 'user' => two_factor_user,
                                               'steps' => [get('/login'), login, get('/verify_two_factor'), otp(code_at('2026-09-10T12:00:00Z'))] },
      'two_factor_next_step_accepted' => { 'user' => two_factor_user,
                                           'steps' => [get('/login'), login, get('/verify_two_factor'), otp(code_at('2026-09-10T12:01:00Z'))] },
      'two_factor_two_steps_back_refused' => { 'user' => two_factor_user,
                                               'steps' => [get('/login'), login, get('/verify_two_factor'), otp(code_at('2026-09-10T11:59:30Z'))] },
      'two_factor_replay_refused' => { 'user' => two_factor_user('last_otp_at' => '2026-09-10T12:00:30Z'),
                                       'steps' => [get('/login'), login, get('/verify_two_factor'), otp(code_at(AT))] },
      'two_factor_fifth_failure_locks' => { 'user' => two_factor_user('failed_attempts' => 4),
                                            'steps' => [get('/login'), login, get('/verify_two_factor'), otp('000000'), get('/login')] },
      'two_factor_locked_password_stage' => { 'user' => two_factor_user('failed_attempts' => 5, 'locked_at' => '2026-09-10T11:59:30Z'),
                                              'steps' => [get('/login'), login, get('/login')] },
      'two_factor_lock_expired' => { 'user' => two_factor_user('failed_attempts' => 5, 'locked_at' => '2026-09-10T11:45:29Z'),
                                     'steps' => [get('/login'), login, get('/verify_two_factor'), otp('000000')] },
      'two_factor_wrong_password' => { 'user' => two_factor_user, 'steps' => [get('/login'), login('wrong')] },
      'two_factor_pending_expired' => { 'user' => two_factor_user,
                                        'steps' => [get('/login'), login, get('/verify_two_factor').merge('advance' => 300), get('/login')] },
      'two_factor_pending_last_second' => { 'user' => two_factor_user,
                                            'steps' => [get('/login'), login, get('/verify_two_factor').merge('advance' => 299)] },
      'two_factor_without_pending' => { 'user' => two_factor_user, 'steps' => [get('/verify_two_factor')] },
      'two_factor_returns_to_root' => { 'user' => two_factor_user,
                                        'steps' => [get('/bots'), get('/login'), login, get('/verify_two_factor'), otp(code_at(AT))] },
      'two_factor_unconfirmed' => { 'user' => two_factor_user('confirmed_at' => nil),
                                    'steps' => [get('/login'), login, get('/verify_two_factor'), otp(code_at(AT)).merge('expect' => 302),
                                                get('/login'), get('/bots')] },
      'two_factor_confirmation_revoked' => { 'user' => two_factor_user,
                                             'steps' => [get('/login'), login, get('/verify_two_factor'),
                                                         otp(code_at(AT)).merge('before' => 'unconfirm', 'expect' => 302), get('/login')] },
      'throttle_login' => { 'steps' => [get('/login')] + Array.new(11) { login('wrong') } },
      'throttle_login_locale_prefix' => { 'steps' => [get('/login')] + Array.new(10) { login('wrong') } + [login('wrong', path: '/de/login/', csrf: 'header')] },
      'throttle_two_factor' => { 'user' => two_factor_user,
                                 'steps' => [get('/login'), login, get('/verify_two_factor')] + Array.new(6) { otp('000000') } },
      'unrouted_delete_login_is_not_throttled' => { 'steps' => [get('/login')] + Array.new(10) { login('wrong') } +
        [post('/login', { '_method' => 'delete' })] },
      'bots_empty' => { 'steps' => [get('/login'), login, get('/bots'), get('/bots'), get('/en/bots')] },
      'bots_empty_hide_balances' => { 'user' => user('hide_balances' => true, 'display_currency' => 'EUR'),
                                      'steps' => [get('/login'), login, get('/bots'), get('/bots')] },
      'bots_empty_de' => { 'user' => user('locale' => 'de'), 'steps' => [get('/login'), login, get('/bots'), get('/de/bots')] },
      'bots_empty_not_admin' => { 'extra_users' => [user('email' => 'second@example.com', 'admin' => false)],
                                  'steps' => [get('/login'), login(email: 'second@example.com'), get('/bots'), get('/bots')] },
      'bots_empty_stocks_active' => { 'app_configs' => { 'market_data_provider' => 'deltabadger' },
                                      'extra_users' => [user('email' => 'second@example.com', 'admin' => false)],
                                      'steps' => [get('/login'), login(email: 'second@example.com'), get('/bots'), get('/bots')] },
      'bots_empty_syncing' => { 'app_configs' => { 'setup_sync_status' => 'in_progress' },
                                'steps' => [get('/login'), login, get('/bots'), get('/bots')] },
      'bots_empty_turbo_frame' => { 'steps' => [get('/login'), login, get('/bots'), get('/bots', 'Turbo-Frame' => 'modal')] },
      'bots_filter_param' => { 'steps' => [get('/login'), login, get('/bots'), get('/bots?filter=archived')] },
      'already_signed_in' => { 'steps' => [get('/login'), login, get('/login'), get('/bots'), get('/bots')] },
      'already_signed_in_de' => { 'steps' => [get('/login'), login, get('/de/login'), get('/de/bots')] },
      'csrf_failure_signed_in' => { 'steps' => [get('/login'), login, get('/bots'), logout.merge('csrf' => 'none'), get('/bots')] },
      'two_factor_after_sign_in' => { 'steps' => [get('/login'), login, get('/verify_two_factor'), get('/bots')] },
      'logout' => { 'user' => user('remember_created_at' => '2026-09-01T00:00:00Z'), 'expect_users' => { 'remember_created_at' => nil },
                    'steps' => [get('/login'), login, get('/bots'), logout.merge('expect' => 303), get('/bots').merge('expect' => 302),
                                get('/login')] },
      'logout_de' => { 'steps' => [get('/de/login'), login(path: '/de/login'), get('/de/bots'), logout('/de/logout').merge('expect' => 303),
                                   get('/de')] },
      'bots_empty_cash_only' => { 'balances' => { 'USD' => 120, 'USDC' => 80 }, 'steps' => [get('/login'), login, get('/bots'), get('/bots')] },
      'unrouted_put_login' => { 'steps' => [get('/login'), post('/login', { '_method' => 'put' })] },
      'unrouted_get_logout' => { 'steps' => [get('/login'), login, get('/logout')] },
      'unrouted_unknown_locale_prefix' => { 'steps' => [get('/zz/login')] },
      'unrouted_locale_before_up' => { 'steps' => [get('/de/up')] },
      'not_ported_tracker' => { 'steps' => [get('/login'), login, get('/tracker')] },
      'not_ported_bots_with_holdings' => { 'balances' => { 'BTC' => 5000, 'USD' => 120 }, 'steps' => [get('/login'), login, get('/bots')] },
      'not_ported_bots_cash_shown' => { 'user' => user('tracker_settings' => { 'show_cash' => true }), 'balances' => { 'USD' => 120 },
                                        'steps' => [get('/login'), login, get('/bots')] },
      'up' => { 'steps' => [get('/up')] }
    }
  end

  def connect(dir) = ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))

  # An install as Rails prepares one, built once and copied per scenario.
  def template(root)
    dir = File.join(root, '.template')
    FileUtils.mkdir_p(dir)
    { 'production.sqlite3' => 'db/schema.rb', 'production_queue.sqlite3' => 'db/queue_schema.rb' }.each do |file, schema|
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, file))
      ActiveRecord::Schema.verbose = false
      load Rails.root.join(schema)
      ActiveRecord::Base.connection_pool.disconnect!
    end
    dir
  end

  def build(dir, template, scenario)
    FileUtils.mkdir_p(dir)
    %w[production.sqlite3 production_queue.sqlite3].each { |file| FileUtils.cp(File.join(template, file), File.join(dir, file)) }
    connect(dir)
    travel_to(Time.iso8601('2026-01-01T00:00:00Z')) do # created_at and updated_at of the seeded rows
      ([scenario['user'] || user] + scenario.fetch('extra_users', [])).each do |attrs|
        times = %w[confirmed_at locked_at last_otp_at remember_created_at].to_h { |column| [column, attrs[column] && Time.iso8601(attrs[column])] }
        User.new(attrs.merge(times).merge('password' => PASSWORD)).save!(validate: false)
      end
      scenario.fetch('app_configs', {}).each { |key, value| AppConfig.set(key, value) }
      balances(scenario.fetch('balances', {}))
    end
    ActiveRecord::Base.connection_pool.disconnect!
  end

  # Priced balances of the first user: symbol => USD value (what the navbar's tracker ring is drawn from).
  def balances(by_symbol)
    return if by_symbol.empty?

    exchange = Exchanges::Kraken.create!(name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4')
    by_symbol.each do |symbol, usd_value|
      asset = Asset.create!(external_id: symbol.downcase, symbol:, name: symbol, category: 'Cryptocurrency')
      AccountBalance.create!(user: User.first, exchange:, asset:, free: 1, locked: 0, usd_price: usd_value, usd_value:,
                             priced_at: Time.current, synced_at: Time.current)
    end
  end

  def selected
    prefixes = ENV['PAGES'].to_s.split(',')
    scenarios.select { |name, _| prefixes.empty? || prefixes.any? { |prefix| name.start_with?(prefix) } }
  end

  def grid(root)
    template = template(root)
    selected.each do |name, scenario|
      dir = File.join(root, name)
      build(dir, template, scenario)
      File.write(File.join(dir, 'scenario.json'), JSON.pretty_generate(
                                                    'page_parity_scratch' => true, 'at' => AT,
                                                    'secret_key_base' => Rails.application.secret_key_base, 'steps' => scenario['steps'],
                                                    'expect_users' => scenario.fetch('expect_users', {})
                                                  ))
    end
    FileUtils.rm_rf(template)
    puts "built #{selected.size} scenarios in #{root}"
  end

  # Things that happen to the install between two requests, outside any browser.
  BEFORE = {
    'change_password' => -> { User.first.update_columns(encrypted_password: User.new(password: 'Another-horse-7').encrypted_password) },
    'unconfirm' => -> { User.first.update_columns(confirmed_at: nil) }
  }.freeze

  def meta_token(page) = page.to_s[/<meta name="csrf-token" content="([^"]+)"/, 1]

  # [opening tag, inner markup] of every form on the page.
  def forms(page) = page.to_s.scan(%r{<form\b([^>]*)>(.*?)</form>}m)

  # The hidden authenticity_token of the first form on `page` that posts to `action`.
  def form_token(page, action)
    forms(page).filter_map { |tag, inner| inner[/name="authenticity_token" value="([^"]+)"/, 1] if tag.include?(%( action="#{action}")) }.first
  end

  # The comparison masks CSRF tokens and stream signatures, and a mask must not hide a broken value. So
  # every token and every signed stream name Rails rendered is checked here, with Rails' own code: the
  # meta tag against the session's token, each form's field against that or the form's own token
  # (Rails issues one per action and method), each stream name against Turbo's verifier.
  def verify_rendered!(where, session)
    body = session.response.body
    controller = session.controller
    genuine = lambda do |token, action, method|
      unmasked = controller.send(:unmask_token, controller.send(:decode_csrf_token, token))
      accepted = [controller.send(:global_csrf_token, session.request.session)]
      accepted << controller.send(:per_form_csrf_token, session.request.session, action.split('?').first.chomp('/'), method) if action
      accepted.any? { |real| ActiveSupport::SecurityUtils.fixed_length_secure_compare(unmasked, real) }
    end
    raise "#{where}: the csrf-token meta tag does not verify" if meta_token(body) && !genuine.(meta_token(body), nil, nil)

    forms(body).each do |tag, inner|
      token = inner[/name="authenticity_token" value="([^"]+)"/, 1] or next
      action = tag[/ action="([^"]*)"/, 1]
      method = (inner[/name="_method" value="([^"]*)"/, 1] || tag[/ method="([^"]*)"/, 1]).upcase
      raise "#{where}: the token of the form for #{method} #{action} does not verify" unless genuine.(token, action, method)
    end
    body.scan(/signed-stream-name="([^"]+)"/).flatten.each do |signed|
      raise "#{where}: a signed stream name does not verify" unless Turbo::StreamsChannel.verified_stream_name(signed)
    end
    # Asset fingerprints are masked too, so each reference is looked up: it must be exactly the path
    # Sprockets gives that asset now.
    body.scan(%r{/assets/([^"'?\s]+)}).flatten.uniq.each do |path|
      logical = path.sub(/-\h{64}(\.[^.\/]+)\z/, '\1')
      built = Rails.application.assets.find_asset(logical)&.digest_path
      raise "#{where}: the page refers to /assets/#{path}, and Sprockets has #{built.inspect}" unless built == path
    end
  end

  def record(root)
    ActionController::Base.allow_forgery_protection = true # config/environments/test.rb turns it off
    Rack::Attack.enabled = true
    Dir[File.join(root, '*/scenario.json')].each do |path|
      dir = File.dirname(path)
      scenario = JSON.parse(File.read(path))
      connect(dir)
      Rack::Attack.cache.store = ActiveSupport::Cache::MemoryStore.new # the test environment's null store counts nothing
      clients = Hash.new do |hash, name| # one browser per client name: its own cookies and the last page it loaded
        session = ActionDispatch::Integration::Session.new(Rails.application)
        session.host! 'localhost:3000'
        hash[name] = { session:, page: nil, content: {} }
      end
      now = Time.iso8601(scenario['at'])
      responses = scenario['steps'].each_with_index.map do |step, index|
        client = clients[step['client'] || 'main']
        now += step['advance'].to_i
        travel_to(now, with_usec: true) do
          BEFORE.fetch(step['before']).call if step['before']
          headers = step['headers'].dup
          params = step['form']&.dup
          if %w[form both].include?(step['csrf'])
            action = step['path'].split('?').first
            params['authenticity_token'] = form_token(client[:page], action) or raise "#{dir} step #{index}: the last page has no form posting to #{action}"
          end
          headers['X-CSRF-Token'] = meta_token(client[:page]) if %w[header both].include?(step['csrf'])
          client[:session].process(step['method'].downcase.to_sym, step['path'], params:, headers:)
        end
        response = client[:session].response
        verify_rendered!("#{dir} step #{index}", client[:session])
        client[:page] = response.body if meta_token(response.body)
        # Rails writes its session cookie on every response; this crate writes it when the session's
        # content changed. So what the session holds after each request is compared with what it held
        # before. session_id is Rails' own bookkeeping and not content.
        content = client[:session].request.session.to_h.except('session_id')
        session_changed = content != client[:content]
        client[:content] = content
        { 'status' => response.status, 'headers' => HEADERS.to_h { |name| [name, response.headers[name]] }.compact, 'body' => response.body,
          'session_changed' => session_changed }
      end
      travel_back
      users = ActiveRecord::Base.connection.select_all("SELECT #{USER_COLUMNS.join(', ')} FROM users ORDER BY id").to_a
      File.write(File.join(dir, 'rails.json'), JSON.pretty_generate('responses' => responses, 'users' => users))
      ActiveRecord::Base.connection_pool.disconnect!
    end
  end
end

# script/rust/oauth.rb loads this file for its helpers and runs its own command.
unless defined?(OAUTH_PARITY)
  command, root = ARGV
  raise ArgumentError, 'usage: grid <root> | record <root>' unless %w[grid record].include?(command) && root

  Pages.public_send(command, root)
end
