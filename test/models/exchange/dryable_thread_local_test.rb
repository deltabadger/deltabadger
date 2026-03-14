require 'test_helper'

class DryableThreadLocalTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:binance_exchange)
    @original_dry_run = Rails.configuration.dry_run
  end

  teardown do
    Thread.current[:force_dry_run] = nil
    Rails.configuration.dry_run = @original_dry_run
  end

  test 'dry_run? returns true when thread-local is set' do
    Rails.configuration.dry_run = false
    Thread.current[:force_dry_run] = true

    assert @exchange.send(:dry_run?)
  end

  test 'dry_run? returns false when thread-local is cleared' do
    Rails.configuration.dry_run = false
    Thread.current[:force_dry_run] = nil

    assert_not @exchange.send(:dry_run?)
  end

  test 'dry_run? returns true when global config is true regardless of thread-local' do
    Rails.configuration.dry_run = true
    Thread.current[:force_dry_run] = nil

    assert @exchange.send(:dry_run?)
  end

  test 'thread-local does not leak across threads' do
    Thread.current[:force_dry_run] = true

    other_thread_value = Thread.new { Thread.current[:force_dry_run] }.value

    assert_nil other_thread_value
  end
end
