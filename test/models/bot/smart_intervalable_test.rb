require 'test_helper'

class Bot::SmartIntervalableTest < ActiveSupport::TestCase
  setup do
    @bot = create(:dca_single_asset, :started)
    @bot.smart_intervaled = true
    @bot.smart_interval_quote_amount = 10.0
    @bot.set_missed_quote_amount
    @bot.save!
  end

  test 'changing quote_amount does not change smart_interval_quote_amount' do
    @bot.quote_amount = @bot.quote_amount * 20
    @bot.set_missed_quote_amount
    @bot.save!

    assert_equal 10.0, @bot.smart_interval_quote_amount
  end
end
