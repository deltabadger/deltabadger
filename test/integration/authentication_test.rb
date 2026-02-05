require 'test_helper'

class AuthenticationTest < ActionDispatch::IntegrationTest
  setup do
    @admin = create(:user, admin: true)
  end

  # == Login ==

  test 'signs in with valid credentials' do
    user = create(:user, password: 'SecurePass1!')

    post user_session_path, params: {
      user: { email: user.email, password: 'SecurePass1!' }
    }

    assert_response :redirect
    follow_redirect!
    assert_equal user, controller.current_user
  end

  test 'rejects invalid password' do
    user = create(:user, password: 'SecurePass1!')

    post user_session_path, params: {
      user: { email: user.email, password: 'wrongpassword' }
    }

    assert_response :unprocessable_content
  end

  test 'rejects unknown email' do
    post user_session_path, params: {
      user: { email: 'unknown@example.com', password: 'SecurePass1!' }
    }

    assert_response :unprocessable_content
  end

  test 'preserves user locale preference on redirect' do
    user = create(:user, password: 'SecurePass1!')
    user.update!(locale: 'pl')

    post user_session_path, params: {
      user: { email: user.email, password: 'SecurePass1!' }
    }

    assert_includes response.location, 'locale=pl'
  end

  # == Login with 2FA ==

  test 'redirects to 2FA verification after password' do
    user = create(:user, password: 'SecurePass1!', otp_module: :enabled)
    user.otp_regenerate_secret
    user.save!

    post user_session_path, params: {
      user: { email: user.email, password: 'SecurePass1!' }
    }

    assert_redirected_to verify_two_factor_path
  end

  test 'does not sign in until 2FA verified' do
    user = create(:user, password: 'SecurePass1!', otp_module: :enabled)
    user.otp_regenerate_secret
    user.save!

    post user_session_path, params: {
      user: { email: user.email, password: 'SecurePass1!' }
    }
    follow_redirect!

    assert_nil controller.current_user
  end

  test 'completes login with valid OTP code' do
    user = create(:user, password: 'SecurePass1!', otp_module: :enabled)
    user.otp_regenerate_secret
    user.save!

    post user_session_path, params: {
      user: { email: user.email, password: 'SecurePass1!' }
    }
    follow_redirect!

    valid_otp = user.otp_code

    post verify_two_factor_path, params: {
      user: { otp_code_token: valid_otp }
    }

    assert_response :redirect
    follow_redirect!
    assert_equal user, controller.current_user
  end

  test 'rejects invalid OTP code' do
    user = create(:user, password: 'SecurePass1!', otp_module: :enabled)
    user.otp_regenerate_secret
    user.save!

    post user_session_path, params: {
      user: { email: user.email, password: 'SecurePass1!' }
    }
    follow_redirect!

    post verify_two_factor_path, params: {
      user: { otp_code_token: '000000' }
    }

    assert_response :unprocessable_content
  end

  # == Logout ==

  test 'signs out the user' do
    user = create(:user)
    sign_in user

    delete destroy_user_session_path

    assert_redirected_to root_path
    follow_redirect!
    assert_nil controller.current_user
  end

  # == Protected routes ==

  test 'redirects unauthenticated users to login' do
    get bots_path

    assert_redirected_to new_user_session_path
  end

  test 'allows authenticated users' do
    user = create(:user)
    sign_in user

    get bots_path

    assert_response :ok
  end
end
