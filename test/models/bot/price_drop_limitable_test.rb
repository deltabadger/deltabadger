require 'test_helper'

class Bot::PriceDropLimitableTest < ActiveSupport::TestCase
  setup do
    @bot = create(:dca_single_asset, :started)
    @bot.price_drop_limited = true
    @bot.price_drop_limit = 0.2
    @bot.price_drop_limit_time_window_condition = 'twenty_four_hours'
    @bot.price_drop_limit_in_ticker_id = @bot.ticker.id
    @bot.set_missed_quote_amount
    @bot.save!
  end

  test 'returns false when not price_drop_limited' do
    @bot.price_drop_limited = false
    result = @bot.get_price_drop_limit_condition_met?
    assert_predicate result, :success?
    assert_equal false, result.data
  end

  test 'returns false when get_high_of_last returns nil data' do
    Ticker.any_instance.stubs(:get_last_price).returns(Result::Success.new(50_000))
    Ticker.any_instance.stubs(:get_high_of_last).returns(Result::Success.new(nil))

    result = @bot.get_price_drop_limit_condition_met?
    assert_predicate result, :success?
    assert_equal false, result.data
  end

  test 'returns true when price has dropped enough' do
    Ticker.any_instance.stubs(:get_last_price).returns(Result::Success.new(75_000))
    Ticker.any_instance.stubs(:get_high_of_last).returns(Result::Success.new(100_000))

    result = @bot.get_price_drop_limit_condition_met?
    assert_predicate result, :success?
    assert_equal true, result.data
  end

  test 'returns false when price has not dropped enough' do
    Ticker.any_instance.stubs(:get_last_price).returns(Result::Success.new(95_000))
    Ticker.any_instance.stubs(:get_high_of_last).returns(Result::Success.new(100_000))

    result = @bot.get_price_drop_limit_condition_met?
    assert_predicate result, :success?
    assert_equal false, result.data
  end

  test 'returns failure when get_last_price fails' do
    Ticker.any_instance.stubs(:get_last_price).returns(Result::Failure.new('API error'))

    result = @bot.get_price_drop_limit_condition_met?
    assert_predicate result, :failure?
  end

  test 'returns failure when get_high_of_last fails' do
    Ticker.any_instance.stubs(:get_last_price).returns(Result::Success.new(50_000))
    Ticker.any_instance.stubs(:get_high_of_last).returns(Result::Failure.new('No candles'))

    result = @bot.get_price_drop_limit_condition_met?
    assert_predicate result, :failure?
  end
end
