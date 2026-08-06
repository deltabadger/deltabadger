require 'test_helper'
require Rails.root.join('db/migrate/20260806001012_rotate_bot_signal_tokens.rb')

# The migration handles a secret column, so its two logging surfaces both need coverage: the
# migration log (Migration#say/say_with_time) and ActiveRecord's own adapter-level SQL log,
# which are silenced by two independent mechanisms and can regress independently.
class RotateBotSignalTokensTest < ActiveSupport::TestCase
  test 'rotates every token to a new, distinct 43-character value' do
    bot = create(:signal_bot)
    first = create(:bot_signal, bot: bot)
    second = create(:bot_signal, bot: bot)
    original_tokens = [first.token, second.token]

    migrate!

    new_tokens = [first.reload.token, second.reload.token]
    assert_equal 2, new_tokens.uniq.size
    new_tokens.each { |token| assert_equal 43, token.length }
    assert_empty original_tokens & new_tokens
  end

  test 'the migration is irreversible' do
    assert_raises(ActiveRecord::IrreversibleMigration) { RotateBotSignalTokens.new.down }
  end

  test 'does not leak a token through ActiveRecord SQL logging at debug level' do
    signal = create(:bot_signal)

    log = capture_ar_debug_log { migrate! }

    refute_includes log, signal.reload.token
    refute_match(/UPDATE bot_signals SET token/, log)
  end

  private

  def migrate!
    RotateBotSignalTokens.new.tap { |m| m.verbose = false }.up
  end

  # ActiveRecord::LogSubscriber#logger always reads ActiveRecord::Base.logger fresh (it is a
  # class_attribute, not memoized), so swapping it here is enough to intercept every SQL log
  # line the adapter emits for the duration of the block, independent of Rails.logger and of
  # whatever level the suite's own logger runs at. ActiveSupport::Logger, not plain ::Logger --
  # every Rails environment logger is one (it's what Rails.logger is built from), and only
  # ActiveSupport::Logger carries the #silence the fix depends on.
  def capture_ar_debug_log
    buffer = StringIO.new
    debug_logger = ActiveSupport::Logger.new(buffer)
    debug_logger.level = Logger::DEBUG
    original_logger = ActiveRecord::Base.logger
    ActiveRecord::Base.logger = debug_logger

    yield

    buffer.string
  ensure
    ActiveRecord::Base.logger = original_logger
  end
end
