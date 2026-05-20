require 'test_helper'

class Bot::ActivityLoggableTest < ActiveSupport::TestCase
  # Lifecycle events are logged ONLY after the state change has persisted, for every
  # bot type (including Signal). A failed start/stop must not leave a false row.

  LIFECYCLE_FACTORIES = %i[dca_single_asset dca_dual_asset signal_bot].freeze

  LIFECYCLE_FACTORIES.each do |factory_name|
    test "#{factory_name}: logs a 'started' activity after a successful start" do
      bot = create(factory_name, :stopped)
      create(:bot_signal, bot: bot) if factory_name == :signal_bot # signal bots require a signal to start

      assert_difference -> { bot.bot_activity_logs.where(event: 'started').count }, 1 do
        bot.start
      end
    end

    test "#{factory_name}: logs a 'stopped' activity after a successful stop" do
      bot = create(factory_name, :started)

      assert_difference -> { bot.bot_activity_logs.where(event: 'stopped').count }, 1 do
        bot.stop
      end
    end
  end

  test 'does not log started when the start does not persist' do
    bot = create(:dca_single_asset, :stopped)
    bot.stubs(:valid?).returns(false)

    assert_no_difference -> { bot.bot_activity_logs.count } do
      bot.start
    end
  end
end
