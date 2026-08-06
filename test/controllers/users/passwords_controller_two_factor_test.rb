require 'test_helper'

class Users::PasswordsControllerTwoFactorTest < ActionDispatch::IntegrationTest
  # The clock is frozen for the whole class. Users::VerifyOtp verifies through ROTP with one
  # step of drift either side, so a code survives one 30-second rollover between ROTP::TOTP#now
  # and the request carrying it, and is refused once a second rollover passes. On a live clock
  # nothing bounds how many a slow run crosses, which would fail a test for a reason that has
  # nothing to do with what it asserts — worst for the tests that carry one code across two
  # request cycles. Rails restores the clock in after_teardown.
  #
  # This does not soften the replay checks below: Users::VerifyOtp records last_otp_at and
  # verifies with `after:`, and ROTP keeps only timecodes strictly greater than that one, so
  # a code consumed by an earlier request is still rejected on the retry even at a standstill.
  setup do
    freeze_time
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

  # Forgetting the password is exactly how an account gets locked, and resetting it is what
  # the app then tells the user to do. Blaming the authenticator for a lock the password
  # stage applied sends them off to re-pair a device that is working fine.
  test 'a locked account is told it is locked, not that its code is wrong' do
    @user.lock_access!

    put '/password', params: {
      user: { reset_password_token: @raw_token, otp_code_token: ROTP::TOTP.new(@user.otp_secret_key).now,
              password: 'New!Password1', password_confirmation: 'New!Password1' }
    }

    assert_response :unprocessable_entity
    assert @user.reload.valid_password?('Old!Password1'), 'the refusal itself must stand'
    assert_includes response.body, I18n.t('devise.failure.locked')
    refute_includes response.body, I18n.t('errors.messages.bad_2fa_code')
  end

  # Devise's own credential path opens with `unlock_access! if lock_expired?`
  # (Devise::Models::Lockable#valid_for_authentication?). Nothing runs it for the counter
  # kept here, so without an explicit reset an expired lock still holds a spent budget and
  # the first wrong code re-locks the account for another unlock_in.
  test 'an expired lock restores the whole attempt budget' do
    Devise.maximum_attempts.times do
      put '/password', params: {
        user: { reset_password_token: @raw_token, otp_code_token: '000000',
                password: 'New!Password1', password_confirmation: 'New!Password1' }
      }
    end
    assert @user.reload.access_locked?

    travel(Devise.unlock_in + 1.minute) do
      put '/password', params: {
        user: { reset_password_token: @raw_token, otp_code_token: '000000',
                password: 'New!Password1', password_confirmation: 'New!Password1' }
      }

      @user.reload
      assert_equal 1, @user.failed_attempts, 'the expired lock must clear the counter before this guess'
      refute @user.access_locked?, 'one wrong code must not re-lock an account whose lock has expired'
    end
  end
end
