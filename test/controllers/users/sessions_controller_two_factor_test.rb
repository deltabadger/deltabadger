require 'test_helper'

class Users::SessionsControllerTwoFactorTest < ActionDispatch::IntegrationTest
  # The clock is frozen for the whole class. Users::VerifyOtp verifies through ROTP with its
  # defaults (drift_ahead: 0, drift_behind: 0), so a code is only accepted inside the exact
  # 30-second step it was minted in, and on a live clock that step can tick over between
  # ROTP::TOTP#now and the request carrying the code. Rails restores the clock in
  # after_teardown, and the expiry test below still travels explicitly from this baseline.
  setup do
    freeze_time
    create(:user, admin: true, setup_completed: true)
    @password = 'Sup3rSecret!pass'
    @user = create(:user, password: @password, setup_completed: true)
    @user.update!(otp_secret_key: ROTP::Base32.random, otp_module: :enabled)
  end

  def start_2fa
    post '/login', params: { user: { email: @user.email, password: @password } }
  end

  def guess(code = '000000')
    post '/verify_two_factor', params: { user: { otp_code_token: code } }
  end

  test 'each wrong code increments the persisted failure counter' do
    start_2fa
    3.times { guess }

    assert_equal 3, @user.reload.failed_attempts
  end

  # The attacker holds the session cookie (:cookie_store), so any in-session counter is
  # theirs to roll back. Re-starting the flow must NOT restore the attempt budget.
  test 'restarting the login flow does not reset the attempt budget' do
    Devise.maximum_attempts.times do
      start_2fa
      guess
    end

    assert @user.reload.access_locked?, 'the account must lock despite a fresh session each time'
  end

  test 'locks the account after Devise.maximum_attempts wrong codes' do
    start_2fa
    Devise.maximum_attempts.times { guess }

    assert @user.reload.access_locked?
  end

  # Pins the boundary in both directions: locking one guess early would silently
  # shrink the budget, and locking one guess late would leave a free attempt.
  test 'the lock trips on the last attempt of the budget and not before' do
    start_2fa
    (Devise.maximum_attempts - 1).times { guess }

    refute @user.reload.access_locked?, 'the budget must not be spent early'

    guess
    assert @user.reload.access_locked?, 'the last attempt of the budget must lock'
  end

  test 'a correct code is refused once the account is locked' do
    start_2fa
    Devise.maximum_attempts.times { guess }

    guess(ROTP::TOTP.new(@user.otp_secret_key).now)
    assert_redirected_to new_user_session_path
    refute @controller.send(:user_signed_in?) if @controller.respond_to?(:user_signed_in?, true)
  end

  test 'a correct code within the budget signs in and clears the counter' do
    start_2fa
    2.times { guess }

    guess(ROTP::TOTP.new(@user.otp_secret_key).now)

    assert_response :redirect
    refute_equal new_user_session_path, response.headers['Location'].to_s.sub(%r{\Ahttps?://[^/]+}, '')
    assert_equal 0, @user.reload.failed_attempts
  end

  test 'a locked account cannot even start the 2FA flow' do
    @user.lock_access!
    start_2fa

    # The absence of a pending id is what proves the password stage refused to hand out
    # a second-factor prompt. The redirect below alone would also be satisfied by the
    # gate inside verify_two_factor.
    assert_nil session[:pending_user_id], 'the password stage must not open a pending sign-in'

    guess(ROTP::TOTP.new(@user.otp_secret_key).now)
    assert_redirected_to new_user_session_path
  end

  test 'the second-factor prompt keeps the account locale when the URL carries none' do
    @user.update!(locale: 'pl')
    start_2fa

    assert_redirected_to verify_two_factor_path(locale: 'pl')
    refute_includes response.headers['Location'].to_s, 'locale=',
                    'the locale belongs in the path segment, not a query param'
  end

  test 'a locale already in the URL wins over the account preference and is not doubled up' do
    @user.update!(locale: 'de')
    post '/pl/login', params: { user: { email: @user.email, password: @password } }

    assert_redirected_to verify_two_factor_path(locale: 'pl')
    refute_includes response.headers['Location'].to_s, 'locale='
  end

  # The route's locale segment is constrained on the way in but not on generation, so a
  # locale the app no longer ships would build a path that 404s on arrival — dead-ending
  # every login for that account, not just one.
  test 'an unroutable account locale falls back to the unprefixed path' do
    refute_includes I18n.available_locales.map(&:to_s), 'xx'
    @user.update!(locale: 'xx')
    start_2fa

    assert_redirected_to verify_two_factor_path
    follow_redirect!
    assert_response :success
  end

  test 'the pending state expires' do
    start_2fa
    travel(Users::SessionsController::PENDING_TTL + 1.minute) do
      guess(ROTP::TOTP.new(@user.otp_secret_key).now)
      assert_redirected_to new_user_session_path
    end
  end
end
