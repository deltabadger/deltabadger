require 'test_helper'

class BotActivityLogTest < ActiveSupport::TestCase
  test 'belongs to a bot and exposes a level enum' do
    bot = create(:dca_single_asset)
    log = bot.bot_activity_logs.create!(event: 'started', level: :info)

    assert_equal bot, log.bot
    assert_predicate log, :info?
  end

  test 'log_activity writes a row with event, message, level and details' do
    bot = create(:dca_single_asset)

    assert_difference -> { bot.bot_activity_logs.count }, 1 do
      bot.log_activity('market_closed', 'closed', level: :info, details: { next_market_open_at: 'soon' })
    end

    log = bot.bot_activity_logs.last
    assert_equal 'market_closed', log.event
    assert_equal 'closed', log.message
    assert_predicate log, :info?
    assert_equal 'soon', log.details['next_market_open_at']
  end

  test 'log_activity defaults to info level' do
    bot = create(:dca_single_asset)
    bot.log_activity('started')

    assert_predicate bot.bot_activity_logs.last, :info?
  end

  test 'log_activity is best-effort: a logging failure never propagates to the caller' do
    bot = create(:dca_single_asset)
    failing = mock('logs')
    failing.stubs(:create!).raises(StandardError.new('boom'))
    bot.stubs(:bot_activity_logs).returns(failing)
    Rails.logger.expects(:warn).once

    assert_nothing_raised do
      assert_nil bot.log_activity('started')
    end
  end
end
