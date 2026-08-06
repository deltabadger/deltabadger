require 'test_helper'
require 'rake'

class EncryptionRakeTest < ActiveSupport::TestCase
  # `load` rather than Rake.application.rake_require: rake_require records the file in
  # $LOADED_FEATURES, so with a fresh Rake::Application per test every run after the first
  # would silently get an application with no tasks defined, and the order would matter.
  setup do
    Rake.application = Rake::Application.new
    Rake::Task.define_task(:environment)
    load Rails.root.join('lib/tasks/encryption.rake')
    @user = create(:user)
    @exchange = create(:binance_exchange)
  end

  teardown do
    Rake.application = nil
    ENV.delete('CONFIRM')
    ENV.delete('CONFIRM_IBKR')
  end

  test 'reset refuses without confirmation and changes nothing' do
    create(:api_key, user: @user, exchange: @exchange)

    assert_raises(SystemExit) { capture_io { Rake::Task['deltabadger:encryption:reset'].invoke } }

    assert_equal 1, ApiKey.count, 'an unconfirmed reset must not touch anything'
  end

  test 'reset runs with confirmation' do
    create(:api_key, user: @user, exchange: @exchange)
    ENV['CONFIRM'] = 'clear-credentials'

    capture_io { Rake::Task['deltabadger:encryption:reset'].invoke }

    assert_equal 0, ApiKey.count
  end

  # Re-registering IBKR's RSA material means going through their portal and waiting days
  # for activation, so it must not go the same way as a two-minute exchange key.
  test 'reset refuses to destroy IBKR credentials without the extra confirmation' do
    create(:api_key, user: @user, exchange: create(:ibkr_exchange), raw_key: 'ibkr-consumer')
    ENV['CONFIRM'] = 'clear-credentials'

    assert_raises(SystemExit) { capture_io { Rake::Task['deltabadger:encryption:reset'].invoke } }

    assert_equal 1, ApiKey.count
  end

  test 'reset destroys IBKR credentials with both confirmations' do
    create(:api_key, user: @user, exchange: create(:ibkr_exchange), raw_key: 'ibkr-consumer')
    ENV['CONFIRM'] = 'clear-credentials'
    ENV['CONFIRM_IBKR'] = 'yes'

    capture_io { Rake::Task['deltabadger:encryption:reset'].invoke }

    assert_equal 0, ApiKey.count
  end

  test 'report prints withdrawal addresses and changes nothing' do
    Rules::Withdrawal.create!(
      user: @user, exchange: @exchange, asset: create(:asset), address: 'wallet-one',
      threshold_type: 'fee_percentage', max_fee_percentage: '1.0', status: :stopped
    )

    out, = capture_io { Rake::Task['deltabadger:encryption:report'].invoke }

    assert_match 'wallet-one', out
    assert_equal 1, Rule.count
  end

  test 'report tells the operator to revoke when the secret was published' do
    create(:api_key, user: @user, exchange: @exchange)
    SecretKeyBaseGuard.stubs(:published?).returns(true)

    out, = capture_io { Rake::Task['deltabadger:encryption:report'].invoke }

    assert_match(/revoke/i, out)
  end

  # This is the split from published-secret-recovery: a privately generated secret that is
  # merely short is not an exposure, and README.md tells that reader to skip revocation.
  # Before this the report unconditionally said "REVOKE" and "anyone holding a copy of this
  # database can still trade with them" — false for this case, and directly contradicting
  # the README it sits next to.
  test 'report does not demand revocation when the secret was never published' do
    create(:api_key, user: @user, exchange: @exchange)
    SecretKeyBaseGuard.stubs(:published?).returns(false)

    out, = capture_io { Rake::Task['deltabadger:encryption:report'].invoke }

    assert_no_match(/revoke/i, out)
    assert_no_match(/anyone/i, out)
  end

  test "reset's confirmation prompt demands revocation when the secret was published" do
    SecretKeyBaseGuard.stubs(:published?).returns(true)

    _out, err = capture_io { assert_raises(SystemExit) { Rake::Task['deltabadger:encryption:reset'].invoke } }

    assert_match(/revoke/i, err)
  end

  # Same split as above, applied to the pre-confirmation warning: before this fix it told
  # every reader to REVOKE every credential, twice, regardless of whether their secret was
  # ever published — walking a private-secret operator through the README's own exemption
  # and then contradicting it.
  test "reset's confirmation prompt does not demand revocation when the secret was never published" do
    SecretKeyBaseGuard.stubs(:published?).returns(false)

    _out, err = capture_io { assert_raises(SystemExit) { Rake::Task['deltabadger:encryption:reset'].invoke } }

    assert_no_match(/revoke/i, err)
  end

  test 'reset does not demand revocation in its post-run summary when the secret was never published' do
    create(:api_key, user: @user, exchange: @exchange)
    ENV['CONFIRM'] = 'clear-credentials'
    SecretKeyBaseGuard.stubs(:published?).returns(false)

    out, = capture_io { Rake::Task['deltabadger:encryption:reset'].invoke }

    assert_no_match(/revoke/i, out)
  end
end
