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

  # The signed-in user of a scenario with bots: the wash-sale question answered with no (an account
  # that has not answered it gets a modal this build does not serve), and a zone that is not UTC.
  def owner(attrs = {}) = user({ 'wash_sale_enabled' => false, 'time_zone' => 'Tallinn' }.merge(attrs))

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
      'bots_empty_with_holdings' => { 'balances' => { 'BTC' => 5000, 'USD' => 120 }, 'steps' => [get('/login'), login, get('/bots')] },
      'bots_empty_cash_shown' => { 'user' => user('tracker_settings' => { 'show_cash' => true }), 'balances' => { 'USD' => 120 },
                                        'steps' => [get('/login'), login, get('/bots')] },
      'up' => { 'steps' => [get('/up')] }
    }.merge(bot_scenarios).merge(action_scenarios)
  end

  def signed_in(*steps) = [get('/login'), login] + steps

  def stopped(kind, spec = {}) = { 'kind' => kind }.merge(spec).merge('columns' => { 'status' => 2 }.merge(spec.fetch('columns', {})))

  # A bot that is scheduled since `started_at`, with the job Rails holds for it at its next checkpoint.
  # A bot at work has acted at its last checkpoint: `acted` is the time Bot::ActionJob wrote then.
  def running(kind, started_at, spec = {})
    { 'kind' => kind, 'job' => 'checkpoint' }.merge(spec).merge('columns' => { 'status' => 1, 'started_at' => started_at }.merge(spec.fetch('columns', {})))
  end

  # The owner's three bots as the wizard leaves them.
  def acted(at, transient = {}) = { 'transient' => { 'last_action_job_at' => at }.merge(transient) }

  def three = [{ 'kind' => 'basket', 'columns' => { 'label' => 'Basket' } }, { 'kind' => 'single' }, { 'kind' => 'index' }]

  def with_bots(bots, steps, attrs = {}) = { 'user' => owner, 'install' => 'alpaca', 'bots' => bots, 'steps' => steps }.merge(attrs)

  # Bot ids follow the order of 'bots'.
  def bot_scenarios = list_scenarios.merge(page_scenarios).merge(feed_scenarios).merge(refused_scenarios)

  # The same three at work: a history of orders and events on the first, a spending cap that has seen one
  # buy on the second, an order resting on the third.
  def traded
    [running('basket', '2026-09-01T12:30:00Z', 'orders' => 'history', 'logs' => 'history', 'transient' => { 'last_action_job_at' => '2026-09-10T11:55:00.250Z' }),
     running('single', '2026-09-08T13:30:00Z', { 'orders' => 'one_fill' }.merge(acted('2026-09-09T13:30:00.800Z', 'quote_amount_limit_enabled_at' => '2026-09-01T00:00:00.000Z'))),
     running('index', '2026-09-07T13:30:00.5Z', { 'orders' => 'open' }.merge(acted('2026-09-07T13:30:01.100Z')))]
  end

  # Stopped bots that may not be started, each for its own reason, and three (7, 8 and 10) that may.
  def blocked
    cap = { 'quote_amount_limit_enabled_at' => '2026-09-01T00:00:00.000Z' }
    [stopped('single', 'settings' => { 'quote_amount_limit' => 40 }, 'orders' => 'one_fill', 'transient' => cap),
     stopped('basket', 'settings' => { 'start_time_enabled' => true, 'start_time_mode' => 'date', 'start_at' => '2026-09-10T12:00:30Z' }),
     stopped('basket', 'settings' => { 'start_time_enabled' => true, 'start_time_mode' => 'friday', 'start_time_of_day' => '25:00' }),
     stopped('index', 'stored' => { 'num_coins' => 1 }),
     stopped('basket', 'stored' => { 'smart_interval_quote_amount' => 0.5 }),
     stopped('basket', 'stored' => { 'price_limited' => true, 'price_limit_in_ticker_id' => 5 }),
     stopped('basket', 'settings' => { 'start_time_enabled' => true, 'start_time_mode' => 'date', 'start_at' => '2026-09-10T12:00:31Z' }),
     stopped('single', 'settings' => { 'quote_amount_limit' => 50.01 }, 'orders' => 'one_fill', 'transient' => cap),
     # The starting time switched on and no mode ever chosen (9): the mode is the error, and the clock time is not looked at.
     stopped('basket', 'settings' => { 'start_time_enabled' => true, 'start_time_of_day' => '25:00' }),
     # A switch that is null is off (10), whatever the rest says.
     stopped('basket', 'settings' => { 'start_time_mode' => 'friday', 'start_time_of_day' => '25:00' }, 'stored' => { 'start_time_enabled' => nil }),
     stopped('single', 'settings' => { 'allocations' => { '3' => 1.0 } }, 'delist' => 'IBIT')]
  end

  # Bots whose tick is due: the checkpoint has come, and Rails' job is no longer scheduled but ready,
  # or blocked behind another job of the venue. Neither side has a time to count down to. The first
  # is at its checkpoint to the microsecond; the fifth was started this second and has not acted yet.
  # The sixth has not acted either, but its first run is still ahead: both sides count down to it.
  # The seventh began its tick within its checkpoint's millisecond. The time it wrote then is cut to
  # the millisecond and so reads as before the checkpoint; it has acted, and both sides count down.
  def due
    [running('single', '2026-09-09T12:00:30.123456Z', { 'job' => 'ready' }.merge(acted('2026-09-09T12:00:30.500Z'))),
     running('index', '2026-09-03T12:00:29Z', { 'job' => 'blocked' }.merge(acted('2026-09-03T12:00:29.400Z'))),
     running('basket', '2026-09-09T12:00:30Z', { 'job' => 'blocked' }.merge(acted('2026-09-10T09:36:30.200Z'))),
     running('single', '2026-09-09T12:00:00Z', { 'job' => 'blocked', 'columns' => { 'status' => 5 }, 'orders' => 'failed' }.merge(acted('2026-09-09T12:00:00.300Z'))),
     running('single', '2026-09-10T12:00:30Z', 'job' => 'blocked'),
     running('single', '2026-09-11T06:30:00Z'),
     running('single', '2026-09-09T12:00:30.000456Z', acted('2026-09-10T12:00:30.000Z'))]
  end

  # A second user with a bot of their own (3), beside the owner's two and one the owner deleted (4).
  def two_users
    { 'user' => owner, 'install' => 'alpaca', 'extra_users' => [owner('email' => 'second@example.com', 'admin' => false)],
      'bots' => [{ 'kind' => 'basket' }, { 'kind' => 'single' }, { 'kind' => 'index', 'owner' => 'second@example.com' },
                 { 'kind' => 'single', 'columns' => { 'status' => 3 } }] }
  end

  # GET /bots with bots.
  def list_scenarios
    holdings = { 'QQQM' => 5000, 'IBIT' => 2500.5, 'USD' => 120, 'NVDA' => 30, 'MSFT' => 20, 'BTC' => 900 }
    {
      'bots_list' => with_bots(three, signed_in(get('/bots'), get('/de/bots'), get('/bots', 'Turbo-Frame' => 'modal'))),
      # One bot in every state the status bar and the button can show, and the four filters over them.
      'bots_list_statuses' => with_bots(
        [running('basket', '2026-09-09T12:30:00Z', 'transient' => { 'last_action_job_at' => '2026-09-10T11:55:00.250Z' }),
         { 'kind' => 'single', 'columns' => { 'status' => 2, 'stop_message_key' => 'bot.settings.extra_amount_limit.amount_spent' },
           'transient' => { 'last_action_job_at' => '2026-09-09T12:30:00.000Z' } },
         running('index', '2026-09-07T13:30:00.5Z', acted('2026-09-07T13:30:01.100Z')),
         { 'kind' => 'basket', 'columns' => { 'status' => 4, 'started_at' => '2026-09-09T12:30:00Z' } },
         running('single', '2026-09-09T12:30:00Z', { 'columns' => { 'status' => 5 }, 'orders' => 'failed' }.merge(acted('2026-09-09T12:30:00.700Z'))),
         { 'kind' => 'single', 'columns' => { 'status' => 6, 'started_at' => '2026-09-09T12:30:00Z' } },
         { 'kind' => 'basket', 'columns' => { 'status' => 7 } },
         { 'kind' => 'index', 'columns' => { 'status' => 3 } },
         { 'kind' => 'basket', 'columns' => { 'status' => 2 } },
         running('single', '2026-09-10T11:59:00Z', { 'columns' => { 'status' => 5 } }.merge(acted('2026-09-10T11:59:00.600Z'))),
         { 'kind' => 'basket', 'settings' => { 'allocations' => { '2' => 0.5, '3' => 0.4 } } }],
        signed_in(get('/bots'), get('/bots?filter=active'), get('/bots?filter=inactive'), get('/bots?filter=archived'), get('/bots?filter=all'),
                  get('/bots?filter=zz'), get('/de/bots?filter=active'))
      ),
      # Exactly one bot: the list is its page.
      'bots_list_single' => with_bots([running('single', '2026-09-08T13:30:00Z', acted('2026-09-09T13:30:00.800Z'))],
                                      signed_in(get('/bots').merge('expect' => 302), get('/de/bots'), get('/bots?filter=archived'))),
      'bots_list_archived_only' => with_bots([{ 'kind' => 'basket', 'columns' => { 'status' => 7 } }, { 'kind' => 'single', 'columns' => { 'status' => 7 } }],
                                             signed_in(get('/bots'), get('/bots?filter=archived'))),
      'bots_list_start_blocked' => with_bots(blocked, signed_in(get('/bots'))),
      'bots_list_key_incorrect' => with_bots(three, signed_in(get('/bots')), 'api_keys' => { 'alpaca' => 'incorrect', 'ibkr' => 'correct' }),
      'bots_list_no_key' => with_bots(three, signed_in(get('/bots')), 'api_keys' => {}),
      'bots_list_of_another_user' => two_users.merge('steps' => signed_in(get('/bots'))),
      # The navbar's tracker icon as a ring of the account's holdings.
      'bots_list_ring' => with_bots(three, signed_in(get('/bots')), 'balances' => holdings),
      'bots_list_ring_with_cash' => with_bots(three, signed_in(get('/bots')), 'user' => owner('tracker_settings' => { 'show_cash' => true }),
                                                                             'balances' => { 'QQQM' => 50, 'USD' => 120 }),
      # A basket of four (its tile names three while they fit) and a basket of coins.
      'bots_list_wide' => with_bots([{ 'kind' => 'wide' }, { 'kind' => 'coins' }, running('wide', '2026-08-31T10:00:00Z', acted('2026-08-31T10:00:00.900Z'))],
                                    signed_in(get('/bots'))),
      # Bots that have traded: the account's total is on its way, and a tile says whether orders are resting.
      'bots_list_traded' => with_bots(traded, signed_in(get('/bots'), get('/de/bots'))),
      'bots_list_hidden' => with_bots(traded, signed_in(get('/bots')), 'user' => owner('hide_balances' => true, 'locale' => 'de')),
      'bots_list_due' => with_bots(due, signed_in(get('/bots')))
    }
  end

  RULE_SETTINGS = %w[smart_intervaled smart_interval_quote_amount limit_ordered limit_order_pcnt_distance quote_amount_limited quote_amount_limit
                     price_limited price_limit price_limit_range_lower_bound price_limit_range_upper_bound price_limit_timing_condition
                     price_limit_value_condition price_limit_in_ticker_id price_drop_limited price_drop_limit price_drop_limit_time_window_condition
                     price_drop_limit_in_ticker_id moving_average_limited moving_average_limit_timing_condition moving_average_limit_value_condition
                     moving_average_limit_in_ticker_id moving_average_limit_in_ma_type moving_average_limit_in_timeframe moving_average_limit_in_period
                     indicator_limited indicator_limit indicator_limit_timing_condition indicator_limit_value_condition indicator_limit_in_ticker_id
                     indicator_limit_in_indicator indicator_limit_in_timeframe num_coins allocation_flattening].freeze

  # Rows from before the rules existed: what each rule's concern supplies on load is not in the row.
  # The first and the last hold only what the wizard asks for. The second has its rules switched on and
  # their values missing. The third is one asset at a whole amount, where the supplied Smart Intervals
  # amount is an Integer's tenth (25 / 10 is 2).
  def older
    on = { 'smart_intervaled' => true, 'price_limited' => true, 'price_drop_limited' => true, 'moving_average_limited' => true,
           'indicator_limited' => true, 'quote_amount_limited' => true, 'limit_ordered' => true }
    [stopped('basket', 'without' => RULE_SETTINGS),
     stopped('basket', 'settings' => on, 'without' => RULE_SETTINGS - on.keys),
     stopped('single', 'settings' => { 'quote_amount' => 25, 'smart_intervaled' => true }, 'without' => RULE_SETTINGS - %w[smart_intervaled]),
     running('index', '2026-09-07T13:30:00.5Z', { 'without' => RULE_SETTINGS }.merge(acted('2026-09-07T13:30:01.100Z')))]
  end

  # Rows no form would have saved, written past the models. A basket whose weights and rebalance
  # threshold are texts (Rails reads them with to_f and to_d). Two stopped bots whose Smart Intervals
  # amount is nothing and less than nothing: Rails prints the floor's error under the field and has
  # no checkpoint to compute. And two at work whose checkpoints are not on the microsecond grid: 7 a
  # day in slices of 1 from 11:00:30.000456 is next due at 14:26:12.857598857…, which Rails' job
  # table holds at …598; the last one's first run is ahead, so its bar is measured from a checkpoint
  # a seventh of a day before its start, which is on no grid either.
  def past_forms
    seventh = { 'quote_amount' => 7, 'smart_intervaled' => true, 'smart_interval_quote_amount' => 1, 'quote_amount_limited' => false }
    [stopped('basket', 'stored' => { 'allocations' => { '2' => '0.6', '3' => '0.4' }, 'rebalance_threshold' => '0.2' }),
     stopped('basket', 'stored' => { 'smart_interval_quote_amount' => 0 }),
     stopped('single', 'stored' => { 'smart_intervaled' => true, 'smart_interval_quote_amount' => -5 }),
     running('single', '2026-09-10T11:00:30.000456Z', { 'settings' => seventh }.merge(acted('2026-09-10T11:00:30.000Z'))),
     running('single', '2026-09-10T18:00:30.000456Z', 'settings' => seventh)]
  end

  # GET /bots/:id and the chart's frame.
  def page_scenarios
    chart = { 'Turbo-Frame' => 'bot_chart' }
    {
      'bot_page_created' => with_bots(three, signed_in(get('/bots/1'), get('/bots/2'), get('/bots/3'), get('/de/bots/1'), get('/de/bots/3'),
                                                       get('/bots/2', 'Turbo-Frame' => 'bot'))),
      'bot_page_running' => with_bots(traded, signed_in(get('/bots/1'), get('/bots/2'), get('/bots/3'), get('/bots/1/chart?metrics_missing=1', chart),
                                                       get('/bots/1/chart'), get('/bots/1.json'), get('/bots/1.turbo_stream'))),
      'bot_page_hidden' => with_bots(traded, signed_in(get('/bots/1'), get('/bots/2'), get('/bots/3'), get('/bots/1/chart', chart)),
                                     'user' => owner('hide_balances' => true, 'locale' => 'de')),
      # Exactly one bot: the navbar says "Bot" and points at it.
      'bot_page_single_bot' => with_bots([running('single', '2026-09-08T13:30:00Z', acted('2026-09-09T13:30:00.800Z'))], signed_in(get('/bots/1'), get('/de/bots/1'))),
      'bot_page_archived' => with_bots([{ 'kind' => 'basket', 'columns' => { 'status' => 7 } }, { 'kind' => 'single', 'columns' => { 'status' => 7 } }],
                                       signed_in(get('/bots/2'))),
      'bot_page_start_blocked' => with_bots(blocked, signed_in(get('/bots/1'), get('/bots/2'), get('/bots/3'), get('/bots/4'), get('/bots/5'), get('/de/bots/5'),
                                                               get('/bots/6'), get('/bots/7'), get('/bots/8'), get('/bots/9'), get('/bots/10'), get('/bots/11'))),
      'bot_page_key_incorrect' => with_bots(three, signed_in(get('/bots/1'), get('/bots/3')), 'api_keys' => { 'alpaca' => 'incorrect', 'ibkr' => 'correct' }),
      'bot_page_no_key' => with_bots(three, signed_in(get('/bots/2')), 'api_keys' => {}),
      'bot_page_wide' => with_bots([{ 'kind' => 'wide' }, { 'kind' => 'coins' }, running('wide', '2026-08-31T10:00:00Z', acted('2026-08-31T10:00:00.900Z'))],
                                   signed_in(get('/bots/1'), get('/bots/2'), get('/bots/3'))),
      # A four-member basket as the Rust engine leaves it mid-run: a leg per member, an unresolved intent on its own
      # ticker, and the engine's own transient_data keys.
      'bot_page_engine_basket' => with_bots([running('wide', '2026-08-31T10:00:00Z', acted('2026-09-09T13:30:00.900Z')).merge('orders' => 'engine_legs')],
                                            signed_in(get('/bots/1'), get('/bots/1.turbo_stream', FEED))),
      # Every rule of a stopped basket switched on, and the three ways of setting a starting time.
      'bot_page_rules' => with_bots(
        [stopped('basket', 'settings' => { 'price_limited' => true, 'price_limit_value_condition' => 'between', 'price_limit_range_lower_bound' => 100.5,
                                           'price_limit_range_upper_bound' => 420, 'price_drop_limited' => true, 'price_drop_limit' => 0.15,
                                           'price_drop_limit_time_window_condition' => 'twenty_four_hours', 'moving_average_limited' => true,
                                           'moving_average_limit_in_ma_type' => 'ema', 'moving_average_limit_in_period' => 21,
                                           'moving_average_limit_in_timeframe' => 'one_week', 'moving_average_limit_timing_condition' => 'after',
                                           'indicator_limited' => true, 'indicator_limit_value_condition' => 'above', 'indicator_limit' => 70.5,
                                           'quote_amount_limited' => true, 'quote_amount_limit' => 500.5, 'smart_intervaled' => false,
                                           'limit_ordered' => false, 'start_time_enabled' => true, 'start_time_mode' => 'date',
                                           'start_at' => '2026-10-01T07:15:00Z', 'price_limit_action' => 'start_selling' },
                           'transient' => { 'quote_amount_limit_enabled_at' => '2026-09-01T00:00:00.000Z', 'last_action_job_at' => '2026-09-09T12:30:00.000Z' },
                           'orders' => 'history'),
         stopped('single', 'settings' => { 'start_time_enabled' => true, 'start_time_mode' => 'hour', 'price_limited' => true, 'price_limit' => 399.99,
                                           'price_limit_timing_condition' => 'after', 'quote_amount_limit' => 40, 'interval' => 'month',
                                           'quote_amount' => 1000, 'smart_intervaled' => true, 'smart_interval_quote_amount' => 950 },
                           'orders' => 'one_fill', 'transient' => { 'quote_amount_limit_enabled_at' => '2026-09-01T00:00:00.000Z' }),
         stopped('index', 'settings' => { 'start_time_enabled' => false, 'start_time_mode' => 'date', 'smart_intervaled' => true,
                                          'smart_interval_quote_amount' => 12.5, 'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.0015,
                                          'num_coins' => 3, 'allocation_flattening' => 1, 'interval' => 'hour' }),
         stopped('index', 'settings' => { 'hold_all' => true, 'num_coins' => 12, 'start_time_enabled' => true, 'start_time_mode' => 'wednesday',
                                          'start_time_of_day' => '07:05' })],
        signed_in(get('/bots/1'), get('/bots/2'), get('/bots/3'), get('/bots/4'), get('/pl/bots/1'), get('/ru/bots/2')),
        'user' => owner('time_zone' => 'Hawaii')
      ),
      # A second broker lists the same two ETFs: a stopped basket may move there, and a broker the user has a key on is marked.
      'bot_page_exchanges' => with_bots([stopped('basket'), stopped('index'), stopped('wide')], signed_in(get('/bots/1'), get('/bots/2'), get('/bots/3')),
                                        'api_keys' => { 'alpaca' => 'correct', 'ibkr' => 'correct' }),
      'bot_page_due' => with_bots(due, signed_in(get('/bots/1'), get('/bots/2'), get('/bots/3'), get('/bots/4'), get('/bots/5'), get('/bots/6'), get('/bots/7'))),
      'bot_page_defaults' => with_bots(older, signed_in(get('/bots'), get('/bots/1'), get('/bots/2'), get('/bots/3'), get('/bots/4'), get('/de/bots/2'))),
      'bot_page_stored_past_forms' => with_bots(past_forms, signed_in(get('/bots'), get('/bots/1'), get('/bots/2'), get('/bots/3'), get('/bots/4'), get('/bots/5'))),
      'bot_page_ring' => with_bots(three, signed_in(get('/bots/1')), 'balances' => { 'QQQM' => 5000, 'IBIT' => 2500.5, 'USD' => 120, 'BTC' => 900 }),
      # Another user's bot, a deleted bot and a number that is no bot's all answer as "no such bot"; `1abc` is bot 1, as Rails reads an id.
      'bot_page_of_another_user' => two_users.merge(
        'steps' => signed_in(get('/bots/3').merge('expect' => 302), get('/bots'), get('/de/bots/3'), get('/de/bots'), get('/bots/4').merge('expect' => 302),
                             get('/bots/99'), get('/bots/abc'), get('/bots/1abc').merge('expect' => 200),
                             get('/bots/%2B1').merge('expect' => 200), get('/bots/%C2%A01').merge('expect' => 302),
                             get('/bots/4/chart', chart).merge('expect' => 200))
      ),
      'bot_page_signed_out' => with_bots(three, [get('/bots/1'), get('/de/bots/1/chart'), get('/login')]),
      # The chart's frame looks its bot up with `find`, which raises for a bot that is not the user's.
      'missing_chart_of_another_users_bot' => two_users.merge('steps' => signed_in(get('/bots/1/chart', chart), get('/bots/3/chart', chart))),
      'missing_chart_of_no_bot' => with_bots(three, signed_in(get('/bots/99/chart'))),
      # Three times that are in no row. Rails holds each in its job table; this build's countdown has
      # the next checkpoint or nothing (rust/tests/pages.rs, `countdown_`).
      'countdown_market_closed' => with_bots([running('single', '2026-09-08T13:30:00Z', 'transient' => { 'waiting_for_market_open' => true },
                                                                                        'job' => '2026-09-10T13:30:00Z'), { 'kind' => 'basket' }],
                                             signed_in(get('/bots/1'), get('/bots'))),
      'countdown_retry_in_progress' => with_bots([running('single', '2026-09-08T13:30:00Z', { 'columns' => { 'status' => 5 }, 'job' => '2026-09-10T12:00:48Z' }
                                                                                                    .merge(acted('2026-09-10T12:00:18.000Z'))),
                                                  { 'kind' => 'basket' }], signed_in(get('/bots/1'), get('/bots'))),
      # Stopped across yesterday's checkpoint and started again without a fresh start: Rails waits for the next
      # checkpoint (Bot::Lifecycle#start, restarting_within_interval?). The row says only that the last checkpoint's
      # tick never began, which is what a bot whose tick is due looks like: the engine of this build would tick at once.
      'countdown_restarted_late' => with_bots([running('single', '2026-09-08T13:30:00Z', acted('2026-09-08T13:30:00.800Z')), { 'kind' => 'basket' }],
                                              signed_in(get('/bots/1'), get('/bots')))
    }
  end

  # What the orders_pagination frame sends when it asks for the next rows of the feed.
  FEED = { 'Turbo-Frame' => 'orders_pagination', 'Accept' => 'text/html, application/xhtml+xml' }.freeze

  # GET /bots/:id.turbo_stream for the orders_pagination frame.
  def feed_scenarios
    by_accept = { 'Turbo-Frame' => 'orders_pagination', 'Accept' => 'text/vnd.turbo-stream.html, text/html, application/xhtml+xml' }
    beyond_buys = [running('basket', '2026-09-01T12:30:00Z', 'orders' => 'beyond_buys'),
                   running('index', '2026-09-07T13:30:00.5Z', 'orders' => 'beyond_buys')]
    {
      # Fourteen orders and eight events, ten rows at a time: the cursor between an event and an order of the same instant, at the end, and unreadable.
      'bot_feed' => with_bots(traded, signed_in(get('/bots/1.turbo_stream', FEED), get('/bots/1.turbo_stream?before=zz', FEED),
                                                get('/bots/1.turbo_stream?before=2026-09-09T13%3A30%3A09.000000Z%7Cactivity%7C3', FEED),
                                                get('/bots/1.turbo_stream?before=2026-09-09T13%3A30%3A09.000000Z%7Ctransaction%7C10', FEED),
                                                get('/bots/1.turbo_stream?before=2026-09-01T09%3A00%3A00.000000Z%7Cactivity%7C1', FEED),
                                                # The cursor's id is read as String#to_i reads it: `+10` and `1_0` are 10, a no-break space is no space,
                                                # and a number past the column's range is still a number.
                                                get('/bots/1.turbo_stream?before=2026-09-09T13%3A30%3A09.000000Z%7Ctransaction%7C%2B10', FEED),
                                                get('/bots/1.turbo_stream?before=2026-09-09T13%3A30%3A09.000000Z%7Ctransaction%7C1_0', FEED),
                                                get('/bots/1.turbo_stream?before=2026-09-09T13%3A30%3A09.000000Z%7Ctransaction%7C%C2%A010', FEED),
                                                get('/bots/1.turbo_stream?before=2026-09-09T13%3A30%3A09.000000Z%7Ctransaction%7C99999999999999999999', FEED),
                                                get('/bots/2.turbo_stream', FEED), get('/bots/3.turbo_stream', FEED), get('/bots/1', by_accept),
                                                get('/bots/1.turbo_stream', 'Turbo-Frame' => 'bot'))),
      'bot_feed_hidden' => with_bots(traded, signed_in(get('/bots/1.turbo_stream', FEED), get('/de/bots/2.turbo_stream', FEED), get('/bots/3.turbo_stream', FEED)),
                                     'user' => owner('hide_balances' => true, 'locale' => 'de')),
      'bot_feed_empty' => with_bots(three, signed_in(get('/bots/1.turbo_stream', FEED))),
      # Sales, liquidations, a redeploy and a rebalance leg: Rails shows them with the same rows as a buy.
      'bot_feed_beyond_buys' => with_bots(beyond_buys, signed_in(get('/bots/1.turbo_stream', FEED), get('/bots/2.turbo_stream', FEED),
                                                                 get('/de/bots/1.turbo_stream', FEED))),
      'bot_feed_beyond_buys_hidden' => with_bots(beyond_buys.take(1), signed_in(get('/bots/1.turbo_stream', FEED)),
                                                 'user' => owner('hide_balances' => true)),
      # The feed has no figures and no modal: Rails answers it whatever the wash-sale rule says, or before it is answered.
      'bot_feed_wash_sale_rule' => with_bots(traded, signed_in(get('/bots/1.turbo_stream', FEED), get('/bots/3.turbo_stream', FEED)),
                                             'user' => owner('wash_sale_enabled' => true, 'wash_sale_jurisdiction' => 'US')),
      'bot_feed_wash_sale_question' => with_bots([stopped('single', 'orders' => 'one_fill'), { 'kind' => 'basket' }],
                                                 signed_in(get('/bots/1.turbo_stream', FEED)), 'user' => owner('wash_sale_enabled' => nil)),
      'bot_feed_of_another_user' => two_users.merge('steps' => signed_in(get('/bots/3.turbo_stream', FEED), get('/bots'))),
      'bot_feed_signed_out' => with_bots(three, [get('/bots/1.turbo_stream', FEED)])
    }
  end

  # Bots Rails serves and this build does not yet: each needs something that is not ported yet.
  def refused_scenarios
    selling = [stopped('single', 'stored' => { 'direction' => 'selling' }), { 'kind' => 'basket' }]
    # 50 a day in slices of 20,000,000: one order every 1,095 years. Rails says so in words; here the calendar is not asked.
    ages = [stopped('basket', 'settings' => { 'smart_interval_quote_amount' => 20_000_000 }), { 'kind' => 'single' }]
    {
      'not_ported_bot_selling' => with_bots(selling, signed_in(get('/bots/2'), get('/bots/1'))),
      'not_ported_bots_list_selling' => with_bots(selling, signed_in(get('/bots'))),
      'not_ported_bot_rebalancing' => with_bots([stopped('basket', 'settings' => { 'rebalance_enabled' => true }), { 'kind' => 'single' }], signed_in(get('/bots/1'))),
      'not_ported_bot_market_cap_weights' => with_bots([stopped('coins', 'settings' => { 'weighting' => 'market_cap' }), { 'kind' => 'single' }],
                                                       signed_in(get('/bots/1'))),
      'not_ported_bot_that_sold' => with_bots([stopped('single', 'orders' => 'sold'), { 'kind' => 'basket' }], signed_in(get('/bots/1'))),
      'not_ported_bot_wash_sale_rule' => with_bots(three, signed_in(get('/bots/1')), 'user' => owner('wash_sale_enabled' => true, 'wash_sale_jurisdiction' => 'US')),
      'not_ported_bot_wash_sale_question' => with_bots([stopped('single', 'orders' => 'one_fill'), { 'kind' => 'basket' }], signed_in(get('/bots/2'), get('/bots/1')),
                                                       'user' => owner('wash_sale_enabled' => nil)),
      'not_ported_bot_a_thousand_years_apart' => with_bots(ages, signed_in(get('/bots/2'), get('/bots/1'))),
      'not_ported_bots_list_a_thousand_years_apart' => with_bots(ages, signed_in(get('/bots')))
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
    jobs = {}
    travel_to(Time.iso8601('2026-01-01T00:00:00Z')) do # created_at and updated_at of the seeded rows
      ([scenario['user'] || user] + scenario.fetch('extra_users', [])).each do |attrs|
        times = %w[confirmed_at locked_at last_otp_at remember_created_at].to_h { |column| [column, attrs[column] && Time.iso8601(attrs[column])] }
        User.new(attrs.merge(times).merge('password' => PASSWORD)).save!(validate: false)
      end
      scenario.fetch('app_configs', {}).each { |key, value| AppConfig.set(key, value) }
      alpaca(scenario) if scenario['install'] == 'alpaca'
      balances(scenario.fetch('balances', {}))
      jobs = scenario.fetch('bots', []).to_h { |spec| [bot(spec).id, spec['job']] }.compact
      action_fixture(scenario)
    end
    ActiveRecord::Base.connection_pool.disconnect!
    jobs
  end

  # Priced balances of the first user: symbol => USD value (what the navbar's tracker ring is drawn from).
  def balances(by_symbol)
    return if by_symbol.empty?

    exchange = Exchanges::Kraken.create!(name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4')
    by_symbol.each do |symbol, usd_value|
      asset = Asset.find_by(symbol:) || Asset.create!(external_id: symbol.downcase, symbol:, name: symbol, category: 'Cryptocurrency')
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
      jobs = build(dir, template, scenario)
      File.write(File.join(dir, 'scenario.json'), JSON.pretty_generate(
                                                    'page_parity_scratch' => true, 'at' => AT,
                                                    'secret_key_base' => Rails.application.secret_key_base, 'steps' => scenario['steps'],
                                                    'expect_users' => scenario.fetch('expect_users', {}), 'jobs' => jobs
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

  # No page of the grid may reach the network: a page that did would be compared on whatever the
  # network said that minute. Every attempt is refused, as an unreachable network refuses it, and
  # written down; rust/tests/pages.rs fails a scenario that made one.
  module NoNetwork
    def initialize(host = nil, *)
      Pages::NETWORK << host.to_s
      raise SocketError, "page parity: no network (#{host})"
    end
  end
  NETWORK = [] # rubocop:disable Style/MutableConstant

  BOT_COLUMNS = %w[id status label position settings transient_data stop_message_key started_at updated_at].freeze

  def record(root)
    ActionController::Base.allow_forgery_protection = true # config/environments/test.rb turns it off
    Rack::Attack.enabled = true
    Rails.configuration.dry_run = false # the test environment's stand-in for an API key: always correct
    TCPSocket.prepend(NoNetwork)
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
      NETWORK.clear
      original_adapter = ActiveJob::Base.queue_adapter
      action_scenario = scenario['steps'].any? { |step| step['action_snapshot'] }
      ActiveJob::Base.queue_adapter = :test if action_scenario
      travel_to(now, with_usec: true) { enqueue_jobs(scenario.fetch('jobs', {})) } # the queue database is one for the whole grid
      responses = scenario['steps'].each_with_index.map do |step, index|
        client = clients[step['client'] || 'main']
        rows_before = other_before = request_exception = nil
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
          if step['action_snapshot']
            rows_before = action_rows
            other_before = action_other_rows
            ActiveJob::Base.queue_adapter.enqueued_jobs.clear
          end
          env = step['action_snapshot'] ? { 'action_dispatch.show_exceptions' => :all } : {}
          capture = ->(*args) { request_exception = args.last[:exception_object] if args.last[:exception_object] }
          ActiveSupport::Notifications.subscribed(capture, 'process_action.action_controller') do
            client[:session].process(step['method'].downcase.to_sym, step['path'], params:, headers:, env:)
          end
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
        answer = { 'status' => response.status, 'headers' => HEADERS.to_h { |name| [name, response.headers[name]] }.compact, 'body' => response.body,
          'session_changed' => session_changed }
        if step['action_snapshot']
          answer.merge!('action_snapshot' => true, 'rows_before' => rows_before, 'rows_after' => action_rows,
                        'other_rows_before' => other_before, 'other_rows_after' => action_other_rows,
                        'jobs' => ActiveJob::Base.queue_adapter.enqueued_jobs.map { |job| job.except(:job).merge('job_class' => job[:job].name) })
          exception = request_exception
          if step['rails_exception_status']
            raise "#{dir} step #{index}: expected Rails exception #{step['rails_exception_status']}, got #{response.status}, #{exception.inspect}" unless exception && response.status == step['rails_exception_status']
            answer['exception'] = { 'class' => exception.class.name, 'message' => exception.message }
            answer['body'] = '' # Exception pages never enter the normal HTML comparator.
          elsif exception
            raise exception
          end
        end
        answer
      end
      ActiveJob::Base.queue_adapter = original_adapter if action_scenario
      travel_back
      users = ActiveRecord::Base.connection.select_all("SELECT #{USER_COLUMNS.join(', ')} FROM users ORDER BY id").to_a
      bots = ActiveRecord::Base.connection.select_all("SELECT #{BOT_COLUMNS.join(', ')} FROM bots ORDER BY id").to_a
      File.write(File.join(dir, 'rails.json'), JSON.pretty_generate('responses' => responses, 'users' => users, 'bots' => bots,
                                                                    'network' => NETWORK.dup))
      ActiveRecord::Base.connection_pool.disconnect!
    end
  end
end

require_relative 'pages_bots'
require_relative 'pages_actions'

# script/rust/oauth.rb loads this file for its helpers and runs its own command.
unless defined?(OAUTH_PARITY)
  command, root = ARGV
  if command == 'figures' && root
    require_relative 'page_figures'
    PageFigures.run(root)
    exit
  end
  raise ArgumentError, 'usage: grid <root> | record <root>' unless %w[grid record].include?(command) && root

  Pages.public_send(command, root)
end
