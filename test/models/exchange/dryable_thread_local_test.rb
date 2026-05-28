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

  # == get_orders shape contract in dry-run mode ==

  test 'get_dry_orders returns { orders:, missing: [] } shape matching the live contract' do
    Rails.configuration.dry_run = true
    base = create(:asset, :bitcoin)
    quote = create(:asset, :usd)
    ticker = create(:ticker, exchange: @exchange, base_asset: base, quote_asset: quote)

    # Rails.cache is :null_store in test — fake the dry-order lookups by stubbing
    # get_dry_order directly so we exercise the wrapper shape, not the cache.
    @exchange.stubs(:get_dry_order).with(order_id: 'dry-order-1')
             .returns(Result::Success.new(ticker: ticker, status: :closed, amount: 0.002))
    @exchange.stubs(:get_dry_order).with(order_id: 'dry-order-2')
             .returns(Result::Success.new(ticker: ticker, status: :closed, amount: 0.003))

    result = @exchange.get_orders(order_ids: %w[dry-order-1 dry-order-2])

    assert result.success?
    assert_equal %i[orders missing].sort, result.data.keys.sort
    assert_equal %w[dry-order-1 dry-order-2].sort, result.data[:orders].keys.sort
    assert_equal [], result.data[:missing]
  end
end
