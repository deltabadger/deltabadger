require 'test_helper'

class Users::VerifyOtpTest < ActiveSupport::TestCase
  # Codes are minted at an offset and submitted "now" rather than travelling the verifier,
  # so the only variable under test is the age of the code — travelling would also move the
  # `after:` baseline and Devise's lock timers.
  #
  # The frozen instant is pinned 15 seconds into a step on purpose. rotp measures drift in
  # SECONDS and applies it to the timestamp before flooring it into a 30-second step, so a
  # clock frozen at a step boundary reaches the adjacent step even with a one-second drift.
  # Freezing mid-step is what makes these assertions discriminate a one-step window from a
  # one-second one instead of flapping with the wall clock.
  MID_STEP = Time.at(1_800_000_015).utc # 1_800_000_015 % 30 == 15

  setup do
    travel_to MID_STEP
    @user = create(:user)
    @user.update!(otp_secret_key: ROTP::Base32.random, otp_module: :enabled)
    @totp = ROTP::TOTP.new(@user.otp_secret_key)
  end

  test 'accepts a code minted one step behind' do
    assert Users::VerifyOtp.call(@user, @totp.at(30.seconds.ago)),
           'a code from the previous step must be accepted'
    assert_equal 30.seconds.ago.to_i / 30 * 30, @user.reload.last_otp_at.to_i,
                 'the matched step, not the moment of verification, is what gets recorded'
  end

  test 'accepts a code minted one step ahead' do
    assert Users::VerifyOtp.call(@user, @totp.at(30.seconds.from_now)),
           'a code from the next step must be accepted'
  end

  test 'rejects a code minted two steps behind' do
    refute Users::VerifyOtp.call(@user, @totp.at(60.seconds.ago)),
           'the window is one step either side, not "some drift"'
    assert_nil @user.reload.last_otp_at, 'a rejected code must not move the OTP clock'
  end

  test 'rejects a code minted two steps ahead' do
    refute Users::VerifyOtp.call(@user, @totp.at(60.seconds.from_now)),
           'the window is one step either side, not "some drift"'
    assert_nil @user.reload.last_otp_at, 'a rejected code must not move the OTP clock'
  end

  # A drift of one interval plus a little looks like a rounding difference but is a cliff: the
  # spare seconds only buy anything at the edges of a step, and what they buy there is a second
  # step of tolerance. These two check the edges, where mid-step assertions cannot tell 30 from 31.
  test 'rejects a code two steps behind when the clock sits on a step boundary' do
    travel_to Time.at(1_800_000_000).utc # 1_800_000_000 % 30 == 0

    refute Users::VerifyOtp.call(@user, @totp.at(60.seconds.ago)),
           'the drift must be exactly one interval, not one interval plus slack'
  end

  test 'rejects a code two steps ahead when the clock sits at the end of a step' do
    travel_to Time.at(1_800_000_029).utc # 1_800_000_029 % 30 == 29

    refute Users::VerifyOtp.call(@user, @totp.at(60.seconds.from_now)),
           'the drift must be exactly one interval, not one interval plus slack'
  end

  test 'refuses a code that has already been spent' do
    code = @totp.now

    assert Users::VerifyOtp.call(@user, code), 'precondition: the current code is accepted once'
    refute Users::VerifyOtp.call(@user, code), 'a spent code must stay spent'
  end

  test 'spending a code from the previous step leaves the current one usable' do
    assert Users::VerifyOtp.call(@user, @totp.at(30.seconds.ago)), 'precondition: a behind code is accepted'

    refute Users::VerifyOtp.call(@user, @totp.at(30.seconds.ago)), 'the behind code itself is spent'
    assert Users::VerifyOtp.call(@user, @totp.now), 'the current code must still work'
  end

  # Accepting an ahead code records the step that code belonged to, and rotp keeps only
  # timecodes strictly greater than the recorded one — so it spends the current step and the
  # next one, not just the current one. Pinned here so the tradeoff cannot be lost in a refactor.
  test 'spending a code from the next step also spends the current step and the one after' do
    assert Users::VerifyOtp.call(@user, @totp.at(30.seconds.from_now)), 'precondition: an ahead code is accepted'

    refute Users::VerifyOtp.call(@user, @totp.now), 'the current step is spent'

    travel 30.seconds
    refute Users::VerifyOtp.call(@user, @totp.now), 'the step the accepted code belonged to is spent too'

    travel 30.seconds
    assert Users::VerifyOtp.call(@user, @totp.now), 'the step after that must be usable again'
  end

  # The only device that can submit a next-step code is one running roughly a step fast, and
  # such a device stops displaying that code the moment real time catches up. The next code it
  # shows is the one after — so the wait is one code rotation, the same as with no drift at all.
  test 'a fast device is not stranded by the step it just spent' do
    assert Users::VerifyOtp.call(@user, @totp.at(30.seconds.from_now)), 'precondition: an ahead code is accepted'

    travel 30.seconds
    assert Users::VerifyOtp.call(@user, @totp.at(30.seconds.from_now)),
           'the next code a fast device displays must be accepted'
  end

  test 'verifying a code against a cleared seed fails instead of raising' do
    user = create(:user)
    User.where(id: user.id).update_all(otp_secret_key: nil)

    assert_nothing_raised do
      assert_not Users::VerifyOtp.call(user.reload, '123456')
    end
  end
end
