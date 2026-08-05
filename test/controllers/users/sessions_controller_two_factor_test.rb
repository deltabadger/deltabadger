require 'test_helper'

class Users::SessionsControllerTwoFactorTest < ActionDispatch::IntegrationTest
  setup do
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

    guess(ROTP::TOTP.new(@user.otp_secret_key).now)
    assert_redirected_to new_user_session_path
  end

  test 'the pending state expires' do
    start_2fa
    travel(Users::SessionsController::PENDING_TTL + 1.minute) do
      guess(ROTP::TOTP.new(@user.otp_secret_key).now)
      assert_redirected_to new_user_session_path
    end
  end
end
