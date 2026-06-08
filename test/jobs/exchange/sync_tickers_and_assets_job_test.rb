require 'test_helper'

class Exchange::SyncTickersAndAssetsJobTest < ActiveSupport::TestCase
  test 'syncs tickers for a crypto exchange' do
    exchange = create(:kraken_exchange)
    MarketData.expects(:sync_tickers!).with(exchange).returns(Result::Success.new)

    Exchange::SyncTickersAndAssetsJob.perform_now(exchange)
  end

  test 'skips stock venues (data-api has no ticker sync for them)' do
    ibkr = create(:ibkr_exchange)
    MarketData.expects(:sync_tickers!).never

    assert_nothing_raised do
      Exchange::SyncTickersAndAssetsJob.perform_now(ibkr)
    end
  end

  test 'raises when the provider reports a failure for a crypto exchange' do
    exchange = create(:kraken_exchange)
    MarketData.stubs(:sync_tickers!).returns(Result::Failure.new('boom'))

    assert_raises(RuntimeError) do
      Exchange::SyncTickersAndAssetsJob.perform_now(exchange)
    end
  end
end
