require 'test_helper'

class BotSignalTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

  setup do
    @bot = create(:signal_bot)
    @signal = create(:bot_signal, bot: @bot)
  end

  test 'generates token automatically on create' do
    signal = BotSignal.new(bot: @bot, direction: :buy, amount: 50)
    assert_nil signal.token
    signal.save!
    assert_not_nil signal.token
    assert_equal 43, signal.token.length # urlsafe_base64(32) produces 43 chars
  end

  test 'mints a high-entropy token' do
    signal = create(:bot_signal, bot: @bot)

    assert signal.token.length >= 43,
           "expected >= 256 bits of token, got #{signal.token.length} chars"
  end

  test 'tokens are unique' do
    tokens = Array.new(20) { create(:bot_signal, bot: @bot).token }
    assert_equal 20, tokens.uniq.length
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

  test 'a percentage rule cannot exceed 100' do
    signal = build(:bot_signal, bot: @bot, amount: 150, amount_type: :percentage)
    assert_not signal.valid?
    assert_predicate signal.errors[:amount], :present?

    signal.amount_type = :fixed
    assert_predicate signal, :valid?
  end

  # The claim is the replay guard: one UPDATE that only one of two concurrent calls can win.
  test 'claim_trigger! admits one call per cooldown' do
    freeze_time do
      assert @signal.claim_trigger!
      assert_equal Time.current, @signal.reload.last_triggered_at
      assert_not @signal.claim_trigger!

      travel BotSignal::TRIGGER_COOLDOWN
      assert @signal.claim_trigger!
    end
  end

  test 'release_trigger! hands back only the claim that was taken' do
    freeze_time do
      earlier = 5.minutes.ago
      @signal.update!(last_triggered_at: earlier)
      assert @signal.claim_trigger!

      @signal.release_trigger!(previous: earlier)
      assert_equal earlier, @signal.reload.last_triggered_at

      assert @signal.claim_trigger!
      newer = 1.minute.from_now
      BotSignal.where(id: @signal.id).update_all(last_triggered_at: newer)
      @signal.release_trigger!(previous: earlier)
      assert_equal newer, @signal.reload.last_triggered_at, 'a newer claim is not ours to release'
    end
  end

  test 'ignore_reason names why a call would do nothing' do
    assert_equal 'bot_not_running', @signal.ignore_reason # the factory bot is :created, not started

    @bot.update!(status: :scheduled)
    assert_nil @signal.ignore_reason

    @signal.update!(enabled: false)
    assert_equal 'signal_disabled', @signal.ignore_reason
  end
end
