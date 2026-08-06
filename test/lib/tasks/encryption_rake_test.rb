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

  test 'report tells the operator to revoke, not merely re-enter' do
    create(:api_key, user: @user, exchange: @exchange)

    out, = capture_io { Rake::Task['deltabadger:encryption:report'].invoke }

    assert_match(/revoke/i, out)
  end
end
