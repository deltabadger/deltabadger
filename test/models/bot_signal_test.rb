require 'test_helper'

class BotSignalTest < ActiveSupport::TestCase
  setup do
    @bot = create(:signal_bot)
    @signal = create(:bot_signal, bot: @bot)
  end

  test 'generates token automatically on create' do
    signal = BotSignal.new(bot: @bot, direction: :buy, amount: 50)
    assert_nil signal.token
    signal.save!
    assert_not_nil signal.token
    assert_equal 6, signal.token.length # urlsafe_base64(4) produces 6 chars
  end

  test 'does not overwrite existing token on create' do
    signal = BotSignal.new(bot: @bot, direction: :buy, amount: 50, token: 'custom-token')
    signal.save!
    assert_equal 'custom-token', signal.token
  end

  test 'validates token uniqueness' do
    duplicate = BotSignal.new(bot: @bot, direction: :buy, amount: 50, token: @signal.token)
    assert_not duplicate.valid?
    assert_includes duplicate.errors[:token], 'has already been taken'
  end

  test 'validates amount is greater than 0' do
    signal = build(:bot_signal, bot: @bot, amount: 0)
    assert_not signal.valid?
    assert_predicate signal.errors[:amount], :present?

    signal.amount = -10
    assert_not signal.valid?

    signal.amount = 0.01
    assert_predicate signal, :valid?
  end

  test 'validates amount presence' do
    signal = build(:bot_signal, bot: @bot, amount: nil)
    assert_not signal.valid?
    assert_predicate signal.errors[:amount], :present?
  end

  test 'validates direction presence' do
    signal = BotSignal.new(bot: @bot, amount: 50, direction: nil)
    # direction enum nil means it's not set
    assert_not signal.valid?
  end

  test 'direction enum works' do
    assert_predicate @signal, :buy?
    @signal.direction = :sell
    assert_predicate @signal, :sell?
  end

  test 'webhook_url returns correct path' do
    assert_equal "/hook/#{@signal.token}", @signal.webhook_url
  end

  test 'belongs to bot' do
    assert_equal @bot, @signal.bot
  end
end
