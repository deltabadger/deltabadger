# A16: signup and password reset (rust/src/web/{registration,password}.rs), on the page oracle.
require 'bcrypt'
# Two step keys, recorded here and replayed by rust/tests/pages.rs:
#   'mails'        => true: the mails the step sent, as [{ to, subject }] (Rails: ActionMailer deliveries, with
#                     deliver_later run inline; Rust: the captured account mailer).
#   'account_rows' => true: every users row through ACCOUNT_SQL (the same text runs on both sides), the password
#                     hash read as the candidate it verifies.
# A path or form value holding __MAILED_TOKEN__ takes the token of the last reset or confirmation link mailed to the
# step's client (any client: the mailbox is shared).
module Pages
  NEW_PASSWORD = 'Another-horse-7'.freeze
  # A reset token the grid seeds as Rails 2.x would have minted it: the digest is Devise's own, under this grid's
  # secret_key_base, so a Rust redemption of RESET_RAW proves the same digest. Not a real token: the test secret.
  RESET_RAW = 'seededResetToken0001'.freeze
  ACCOUNT_PASSWORDS = [PASSWORD, NEW_PASSWORD].freeze

  def self.reset_digest = Devise.token_generator.digest(User, :reset_password_token, RESET_RAW)

  # Random values read as their kind: 'seeded' for the grid's own, 'minted' (and the length) for a fresh one.
  def self.account_sql
    <<~SQL.squish
      SELECT id, name, email, encrypted_password, admin, setup_completed, locale, time_zone, display_currency,
        CASE WHEN confirmation_token IS NULL THEN NULL ELSE 'minted:' || length(confirmation_token) END AS confirmation_token,
        confirmation_sent_at, confirmed_at, unconfirmed_email,
        CASE WHEN reset_password_token IS NULL THEN NULL WHEN reset_password_token = '#{reset_digest}' THEN 'seeded'
             ELSE 'minted:' || length(reset_password_token) END AS reset_password_token,
        reset_password_sent_at, failed_attempts, locked_at, otp_module, otp_secret_key IS NOT NULL AS otp_seeded,
        last_otp_at, remember_created_at, created_at, updated_at
      FROM users ORDER BY id
    SQL
  end

  def self.account_rows(sql)
    ActiveRecord::Base.connection.select_all(sql).to_a.each do |row|
      hash = row['encrypted_password']
      row['encrypted_password'] = ACCOUNT_PASSWORDS.find { |candidate| BCrypt::Password.new(hash) == candidate } || 'other'
    end
  end

  # The token in a mailed reset or confirmation link.
  def self.mailed_token(html) = html.to_s[/(?:reset_password|confirmation)_token=([A-Za-z0-9_-]+)/, 1]

  def self.with_mailed_token(value, token)
    return value unless value.is_a?(String) && value.include?('__MAILED_TOKEN__')
    raise 'no mailed token to use' unless token

    value.gsub('__MAILED_TOKEN__', token)
  end

  module AccountsGrid
    def scenarios
      super.merge(account_scenarios)
    end
  end

  def self.signup(name: 'Ann Example', email: 'new@example.com', password: PASSWORD, path: '/signup', **opts)
    post(path, { 'user[name]' => name, 'user[email]' => email, 'user[password]' => password },
         { 'mails' => true, 'account_rows' => account_sql }.merge(opts.transform_keys(&:to_s)))
  end

  def self.reset_request(email, path: '/password', **opts)
    post(path, { 'user[email]' => email }, { 'mails' => true, 'account_rows' => account_sql }.merge(opts.transform_keys(&:to_s)))
  end

  def self.reset_update(password, confirmation = password, token: RESET_RAW, code: nil, path: '/password', **opts)
    form = { '_method' => 'patch', 'user[reset_password_token]' => token, 'user[password]' => password,
             'user[password_confirmation]' => confirmation }
    form['user[otp_code_token]'] = code unless code.nil?
    post(path, form, { 'mails' => true, 'account_rows' => account_sql }.merge(opts.transform_keys(&:to_s)))
  end

  def self.edit(token = RESET_RAW, prefix: '') = get("#{prefix}/password/edit?reset_password_token=#{token}")

  def self.reset_user(attrs = {}) = user({ 'reset_password_token' => reset_digest, 'reset_password_sent_at' => '2026-09-10T11:00:00Z' }.merge(attrs))
  def self.reset_two_factor_user(attrs = {}) = reset_user({ 'otp_module' => 1, 'otp_secret_key' => OTP_SEED }.merge(attrs))

  OPEN = { 'app_configs' => { 'registration_open' => 'true' } }.freeze
  # Exactly reset_password_within before AT, and a microsecond more.
  RESET_BOUNDARY = '2026-09-10T06:00:30.123456Z'.freeze
  RESET_EXPIRED = '2026-09-10T06:00:30.123455Z'.freeze

  def self.account_scenarios
    grid = {}
    # Signup, registration open.
    %w[en de ru].each { |locale| grid["signup_page_#{locale}"] = OPEN.merge('steps' => [get(locale == 'en' ? '/signup' : "/#{locale}/signup")]) }
    {
      'success' => [signup(expect: 303)],
      'success_normalised' => [signup(email: '  NEW@Example.com ')],
      'taken' => [signup(email: 'owner@example.com')],
      'taken_normalised' => [signup(email: ' Owner@EXAMPLE.com ')],
      'taken_weak_password' => [signup(email: 'owner@example.com', password: 'simple', expect: 422)],
      'blank' => [signup(name: '', email: '', password: '', expect: 422)],
      'bad_name' => [signup(name: 'Ann 9', expect: 422)],
      'bad_email' => [signup(email: 'not-an-email', expect: 422)],
      'weak_password' => [signup(password: 'simple', expect: 422)],
      'long_password' => [signup(password: "Aa1!#{'x' * 130}", expect: 422)],
      'markup_name' => [signup(name: '<b>Ann</b>', email: 'a"b<c>@example.com', expect: 422)],
      'no_user' => [post('/signup', {}, 'account_rows' => account_sql)],
      'csrf_failure' => [signup(csrf: 'none')]
    }.each { |name, steps| grid["signup_#{name}"] = OPEN.merge('steps' => [get('/signup')] + steps) }
    grid['signup_success_de'] = OPEN.merge('steps' => [get('/de/signup'), signup(path: '/de/signup')])
    grid['signup_taken_gmail_alias'] = OPEN.merge('extra_users' => [user('name' => 'Ann', 'email' => 'ann@gmail.com', 'admin' => false)],
                                                  'steps' => [get('/signup'), signup(email: 'ann+new@googlemail.com')])
    grid['signup_signed_in'] = OPEN.merge('steps' => [get('/login'), login, get('/signup')])
    # Registration closed (the default).
    grid['signup_closed_page'] = { 'steps' => [get('/signup'), get('/de/signup')] }
    grid['signup_closed_post'] = { 'steps' => [get('/login'), signup(csrf: 'header')] }

    # Requesting a reset.
    %w[en de ru].each { |locale| grid["password_new_page_#{locale}"] = { 'steps' => [get(locale == 'en' ? '/password/new' : "/#{locale}/password/new")] } }
    {
      'known' => 'owner@example.com', 'unknown' => 'nobody@example.com', 'blank' => '', 'normalised' => '  OWNER@Example.com '
    }.each do |name, email|
      grid["password_request_#{name}"] = { 'steps' => [get('/password/new'), reset_request(email, expect: 303), get('/login')] }
    end
    grid['password_request_unconfirmed'] = { 'user' => user('confirmed_at' => nil), 'steps' => [get('/password/new'), reset_request('owner@example.com')] }
    grid['password_request_de'] = { 'steps' => [get('/de/password/new'), reset_request('owner@example.com', path: '/de/password'), get('/de/login')] }
    grid['password_request_no_user'] = { 'steps' => [get('/password/new'), post('/password', {}, 'account_rows' => account_sql)] }
    grid['password_request_csrf_failure'] = { 'steps' => [get('/password/new'), reset_request('owner@example.com', csrf: 'none')] }
    grid['password_new_signed_in'] = { 'steps' => [get('/login'), login, get('/password/new')] }

    # The reset page.
    grid['password_edit_valid'] = { 'user' => reset_user, 'steps' => [edit] }
    grid['password_edit_valid_de'] = { 'user' => reset_user, 'steps' => [edit(prefix: '/de')] }
    grid['password_edit_two_factor'] = { 'user' => reset_two_factor_user, 'steps' => [edit] }
    grid['password_edit_boundary'] = { 'user' => reset_user('reset_password_sent_at' => RESET_BOUNDARY), 'steps' => [edit] }
    grid['password_edit_expired'] = { 'user' => reset_user('reset_password_sent_at' => RESET_EXPIRED), 'steps' => [edit, get('/password/new')] }
    grid['password_edit_unknown'] = { 'user' => reset_user, 'steps' => [edit('wrong-token'), get('/password/new')] }
    grid['password_edit_no_token'] = { 'user' => reset_user, 'steps' => [get('/password/edit'), get('/login')] }
    grid['password_edit_signed_in'] = { 'user' => reset_user, 'steps' => [get('/login'), login, edit] }

    # Setting the new password, an account without two-factor.
    {
      'success' => [reset_update(NEW_PASSWORD, expect: 303), get('/login'), login(NEW_PASSWORD)],
      'bad_password' => [reset_update('simple', expect: 422)],
      'mismatch' => [reset_update(NEW_PASSWORD, 'Different-horse-8', expect: 422)],
      'blank' => [reset_update('', expect: 422)],
      'unknown_token' => [reset_update(NEW_PASSWORD, token: 'wrong-token', expect: 422)],
      'blank_token' => [reset_update(NEW_PASSWORD, token: '', expect: 422)],
      'replayed' => [reset_update(NEW_PASSWORD, expect: 303), edit, get('/password/new')]
    }.each { |name, steps| grid["password_update_#{name}"] = { 'user' => reset_user, 'steps' => [edit] + steps } }
    grid['password_update_success_de'] = { 'user' => reset_user, 'steps' => [edit(prefix: '/de'), reset_update(NEW_PASSWORD, path: '/de/password'), get('/de/login')] }
    grid['password_update_expired'] = { 'user' => reset_user('reset_password_sent_at' => RESET_EXPIRED),
                                        'steps' => [get('/password/new'), reset_update(NEW_PASSWORD, csrf: 'header', expect: 422)] }
    grid['password_update_locked'] = { 'user' => reset_user('failed_attempts' => 5, 'locked_at' => '2026-09-10T11:59:00Z'),
                                       'steps' => [edit, reset_update(NEW_PASSWORD), get('/login'), login(NEW_PASSWORD)] }
    grid['password_update_unconfirmed'] = { 'user' => reset_user('confirmed_at' => nil), 'steps' => [edit, reset_update(NEW_PASSWORD), get('/login')] }

    # Setting the new password on a two-factor account, in Users::PasswordsController#update's order.
    good = code_at(AT)
    bad = good == '000000' ? '111111' : '000000'
    locked = { 'failed_attempts' => 5, 'locked_at' => '2026-09-10T11:59:00Z' }
    lapsed = { 'failed_attempts' => 5, 'locked_at' => '2026-09-10T11:45:00Z' }
    {
      'good_code' => [{}, [reset_update(NEW_PASSWORD, code: good, expect: 303), get('/login')]],
      'good_code_counter' => [{ 'failed_attempts' => 3 }, [reset_update(NEW_PASSWORD, code: good)]],
      'blank_code' => [{}, [reset_update(NEW_PASSWORD, code: '', expect: 422)]],
      'no_code' => [{}, [reset_update(NEW_PASSWORD, expect: 422)]],
      'bad_code' => [{ 'failed_attempts' => 1 }, [reset_update(NEW_PASSWORD, code: bad, expect: 422)]],
      'lock_reached' => [{ 'failed_attempts' => 4 }, [reset_update(NEW_PASSWORD, code: bad, expect: 422), reset_update(NEW_PASSWORD, code: good, expect: 422)]],
      'locked' => [locked, [reset_update(NEW_PASSWORD, code: good, expect: 422)]],
      'lock_expired_good_code' => [lapsed, [reset_update(NEW_PASSWORD, code: good, expect: 303)]],
      'lock_expired_blank_code' => [lapsed, [reset_update(NEW_PASSWORD, code: '', expect: 422)]],
      'lock_expired_blank_password' => [lapsed, [reset_update('', code: good, expect: 422)]],
      'lock_expired_bad_password' => [lapsed, [reset_update('simple', code: good, expect: 422)]],
      'lock_expired_bad_code' => [lapsed, [reset_update(NEW_PASSWORD, code: bad, expect: 422)]],
      'blank_password' => [{}, [reset_update('', code: good, expect: 422)]],
      'bad_password' => [{}, [reset_update('simple', code: good, expect: 422)]],
      'mismatch' => [{}, [reset_update(NEW_PASSWORD, 'Different-horse-8', code: good, expect: 422)]],
      'unknown_token' => [{}, [reset_update(NEW_PASSWORD, token: 'wrong-token', code: good, expect: 422)]]
    }.each do |name, (attrs, steps)|
      grid["password_2fa_#{name}"] = { 'user' => reset_two_factor_user(attrs), 'steps' => [edit] + steps }
    end
    grid['password_2fa_expired_token'] = { 'user' => reset_two_factor_user('reset_password_sent_at' => RESET_EXPIRED),
                                           'steps' => [get('/password/new'), reset_update(NEW_PASSWORD, code: good, csrf: 'header', expect: 422)] }
    # The sign-in and the reset share one failure counter: four wrong passwords, then one wrong code locks.
    grid['password_2fa_counter_shared'] = { 'user' => reset_two_factor_user,
                                            'steps' => [get('/login')] + Array.new(4) { login('wrong') } +
                                                       [edit.merge('client' => 'mail'), reset_update(NEW_PASSWORD, code: bad, client: 'mail')] }

    # The whole flow on the mailed link.
    grid['password_flow_mailed'] = { 'steps' => [get('/password/new'), reset_request('owner@example.com'), edit('__MAILED_TOKEN__'),
                                                 reset_update(NEW_PASSWORD, token: '__MAILED_TOKEN__'), get('/login'), login(NEW_PASSWORD)] }
    grid['password_flow_mailed_two_factor'] = { 'user' => two_factor_user,
                                                'steps' => [get('/password/new'), reset_request('owner@example.com'), edit('__MAILED_TOKEN__'),
                                                            reset_update(NEW_PASSWORD, token: '__MAILED_TOKEN__', code: good)] }

    # A reset link dies with a change of password or email (Devise's clear_reset_password_token).
    signed = [get('/login'), login, get('/settings/account')]
    change = lambda do |action, form|
      post("/settings/#{action}", { '_method' => 'patch' }.merge(form), 'csrf' => 'header', 'headers' => TURBO, 'account_rows' => account_sql)
    end
    password_change = change.call('update_password', { 'user[password]' => NEW_PASSWORD, 'user[password_confirmation]' => NEW_PASSWORD,
                                                       'user[current_password]' => PASSWORD })
    email_change = ->(email) { change.call('update_email', { 'user[email]' => email, 'user[current_password]' => PASSWORD }) }
    anonymous = ->(step) { step.merge('client' => 'anonymous') }
    grid['password_token_after_password_change'] = { 'user' => reset_user(owner), 'steps' => signed + [password_change, anonymous[edit]] }
    grid['password_token_after_email_change_request'] = { 'user' => reset_user(owner), 'steps' => signed + [email_change['next@example.com'], anonymous[edit]] }
    grid['password_token_after_email_confirmation'] = {
      'user' => owner,
      'steps' => signed + [email_change['next@example.com'], anonymous[get('/password/new')], anonymous[reset_request('owner@example.com')],
                           get('/confirmation?confirmation_token=__R2_TOKEN__').merge('confirmation_token_from_user' => true, 'account_rows' => account_sql),
                           anonymous[edit('__MAILED_TOKEN__')]]
    }
    %w[invalid owner@example.com].each_with_index do |email, index|
      grid["password_token_kept_after_refused_email_change_#{index}"] = { 'user' => reset_user(owner), 'steps' => signed + [email_change[email], anonymous[edit]] }
    end
    grid['password_token_kept_after_taken_email_change'] = { 'user' => reset_user(owner), 'extra_users' => [user('name' => 'Other', 'email' => 'other@example.com', 'admin' => false)],
                                                             'steps' => signed + [email_change['other@example.com'], anonymous[edit]] }
    grid
  end

  singleton_class.prepend(AccountsGrid)
end
