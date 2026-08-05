require 'test_helper'

class Users::PasswordsControllerTwoFactorTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true)
    @user = create(:user, password: 'Old!Password1', setup_completed: true)
    @user.update!(otp_secret_key: ROTP::Base32.random, otp_module: :enabled)
    @raw_token = @user.send_reset_password_instructions
  end

  # A fresh account has never spent a code, so its OTP clock starts nil and only moves
  # if Users::VerifyOtp ran. assert_equal refuses a nil expectation, so assert both ends
  # instead — pinning the baseline too, in case setup ever stops starting from nil.
  def assert_otp_clock_unchanged(before)
    assert_nil before, 'baseline: the account must start with an unspent OTP clock'
    assert_nil @user.reload.last_otp_at, 'the code must not have been consumed'
  end

  test 'does not change the password when the OTP is missing' do
    put '/password', params: {
      user: { reset_password_token: @raw_token,
              password: 'New!Password1', password_confirmation: 'New!Password1' }
    }

    assert_response :unprocessable_entity
    assert @user.reload.valid_password?('Old!Password1'), 'password must be unchanged'
  end

  test 'does not consume the reset token when the OTP is missing' do
    put '/password', params: {
      user: { reset_password_token: @raw_token,
              password: 'New!Password1', password_confirmation: 'New!Password1' }
    }

    assert @user.reload.reset_password_token.present?, 'token must survive a failed attempt'
  end

  test 'does not change the password when the OTP is wrong' do
    put '/password', params: {
      user: { reset_password_token: @raw_token, otp_code_token: '000000',
              password: 'New!Password1', password_confirmation: 'New!Password1' }
    }

    assert_response :unprocessable_entity
    assert @user.reload.valid_password?('Old!Password1')
  end

  # This is also the ONLY live guard on the `user.reload` in the pre-check. Users::VerifyOtp
  # calls user.update, which persists every dirty attribute, so an un-reloaded record carries
  # the assigned password into that write. Devise's Recoverable then clears the reset token in
  # a before_update — encrypted_password changed while a token was present — and the token
  # lookup in `super` no longer matches. The account ends up with the NEW password on a request
  # that reports failure.
  #
  # A separate state-only test for that is not possible, so don't add one: every failing branch
  # reloads before anything can flush the dirty password (Devise's increment_failed_attempts is
  # increment_counter plus reload, and Users::VerifyOtp returns false before its update on a bad
  # code), which leaves the successful reset as the only reachable case — and there the persisted
  # end state is identical with or without the reload. The response status is the only observable
  # that separates them, which is why the message below carries the diagnosis.
  test 'changes the password with a correct OTP' do
    code = ROTP::TOTP.new(@user.otp_secret_key).now
    put '/password', params: {
      user: { reset_password_token: @raw_token, otp_code_token: code,
              password: 'New!Password1', password_confirmation: 'New!Password1' }
    }

    assert_response :redirect,
                    'a successful reset must redirect, not re-render the form. A 422 here means ' \
                    'the new password was written through Users::VerifyOtp before Devise validated ' \
                    'the token, clearing the token so `super` found nothing: the account has been ' \
                    'changed while the user is told the request failed'
    @user.reload
    assert @user.valid_password?('New!Password1')
    assert_nil @user.reset_password_token, 'Devise must consume the token on success'
  end

  # A request that omits the password key leaves password_required? false on a persisted
  # record, so `valid?` waves it through — and the code would be spent on a request Devise
  # rejects as blank immediately afterwards.
  test 'does not consume the OTP when the request carries no password at all' do
    before_otp_at = @user.last_otp_at
    code = ROTP::TOTP.new(@user.otp_secret_key).now

    put '/password', params: { user: { reset_password_token: @raw_token, otp_code_token: code } }

    assert @user.reload.valid_password?('Old!Password1')
    assert_otp_clock_unchanged(before_otp_at)

    # The same code must still work once a password is actually supplied.
    put '/password', params: {
      user: { reset_password_token: @raw_token, otp_code_token: code,
              password: 'New!Password1', password_confirmation: 'New!Password1' }
    }
    assert @user.reload.valid_password?('New!Password1')
  end

  # `update` now intercepts every reset, 2FA or not, so the ordinary path needs a guard of
  # its own: a regression in user_from_reset_token or the otp_module_enabled? check would
  # break password reset for the majority of accounts.
  test 'an account without 2FA can still complete a password reset' do
    plain_user = create(:user, password: 'Old!Password1', setup_completed: true)
    token = plain_user.send_reset_password_instructions

    put '/password', params: {
      user: { reset_password_token: token,
              password: 'New!Password1', password_confirmation: 'New!Password1' }
    }

    assert_response :redirect
    plain_user.reload
    assert plain_user.valid_password?('New!Password1')
    assert_nil plain_user.reset_password_token, 'Devise must consume the token on success'
  end

  # Users::VerifyOtp advances last_otp_at, so a code checked before Devise validates
  # would be spent on a request that was going to fail anyway.
  test 'does not consume the OTP when the new password fails policy' do
    before_otp_at = @user.last_otp_at
    code = ROTP::TOTP.new(@user.otp_secret_key).now
    put '/password', params: {
      user: { reset_password_token: @raw_token, otp_code_token: code,
              password: 'short', password_confirmation: 'short' }
    }

    assert @user.reload.valid_password?('Old!Password1')
    assert_otp_clock_unchanged(before_otp_at)

    # The same code must still work on a well-formed retry.
    put '/password', params: {
      user: { reset_password_token: @raw_token, otp_code_token: code,
              password: 'New!Password1', password_confirmation: 'New!Password1' }
    }
    assert @user.reload.valid_password?('New!Password1')
  end

  test 'does not consume the OTP when the confirmation does not match' do
    before_otp_at = @user.last_otp_at
    code = ROTP::TOTP.new(@user.otp_secret_key).now
    put '/password', params: {
      user: { reset_password_token: @raw_token, otp_code_token: code,
              password: 'New!Password1', password_confirmation: 'Different!1' }
    }

    @user.reload
    assert @user.valid_password?('Old!Password1')
    # The name of this test is about the OTP, so assert the OTP clock — not just the
    # password, which would stay green even if VerifyOtp had spent the code.
    assert_otp_clock_unchanged(before_otp_at)
    assert_equal 0, @user.failed_attempts, 'a password mismatch is not an OTP failure'
  end

  test 'an expired reset token is refused without consuming the OTP' do
    @user.update!(reset_password_sent_at: (Devise.reset_password_within + 1.hour).ago)
    before_otp_at = @user.last_otp_at
    code = ROTP::TOTP.new(@user.otp_secret_key).now

    put '/password', params: {
      user: { reset_password_token: @raw_token, otp_code_token: code,
              password: 'New!Password1', password_confirmation: 'New!Password1' }
    }

    @user.reload
    assert @user.valid_password?('Old!Password1')
    assert_otp_clock_unchanged(before_otp_at)
  end

  test 'repeated wrong OTPs lock the account' do
    Devise.maximum_attempts.times do
      put '/password', params: {
        user: { reset_password_token: @raw_token, otp_code_token: '000000',
                password: 'New!Password1', password_confirmation: 'New!Password1' }
      }
    end

    assert @user.reload.access_locked?
    assert @user.valid_password?('Old!Password1')
  end

  test 'a correct OTP is refused once the account is locked' do
    Devise.maximum_attempts.times do
      put '/password', params: {
        user: { reset_password_token: @raw_token, otp_code_token: '000000',
                password: 'New!Password1', password_confirmation: 'New!Password1' }
      }
    end

    put '/password', params: {
      user: { reset_password_token: @raw_token, otp_code_token: ROTP::TOTP.new(@user.otp_secret_key).now,
              password: 'New!Password1', password_confirmation: 'New!Password1' }
    }

    assert @user.reload.valid_password?('Old!Password1'),
           'the lock must hold on the reset path, not only on sign-in'
  end
end
