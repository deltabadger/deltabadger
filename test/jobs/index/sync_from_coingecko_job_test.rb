require 'test_helper'

# The daily index pull is what changes an index bot's members, so every index bot that can still act
# re-checks them straight after it. Before, a bot caught up only at its next buy, rebalance or sale, and
# meanwhile offered to sell, as quitters, names that were back in the index days earlier.
class Index::SyncFromCoingeckoJobTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper

  # The app pins the SolidQueue adapter; ask for the test one so enqueues can be asserted.
  def queue_adapter_for_test
    ActiveJob::QueueAdapters::TestAdapter.new
  end

  setup do
    MarketData.stubs(:configured?).returns(true)
    MarketDataSettings.stubs(:deltabadger?).returns(true)
    @exchange = create(:kraken_exchange)
    @quote = create(:asset, :eur)
  end

  test 'after the pull, every index bot that can still act re-checks its members' do
    MarketData.stubs(:sync_indices_from_deltabadger!).returns(Result::Success.new)
    acting = %i[created scheduled waiting stopped].map { |status| index_bot(status) }
    gone = %i[deleted archived].map { |status| index_bot(status) }
    other = create(:dca_single_asset)

    acting.each { |bot| Bot::ResyncIndexCompositionJob.expects(:perform_later).with(bot) }
    [*gone, other].each { |bot| Bot::ResyncIndexCompositionJob.expects(:perform_later).with(bot).never }

    Index::SyncFromCoingeckoJob.perform_now
  end

  test 'a failed pull re-checks nothing and is tried again' do
    MarketData.stubs(:sync_indices_from_deltabadger!).returns(Result::Failure.new('upstream down'))
    index_bot(:scheduled)
    Bot::ResyncIndexCompositionJob.expects(:perform_later).never

    assert_enqueued_with(job: Index::SyncFromCoingeckoJob) { Index::SyncFromCoingeckoJob.perform_now }
  end

  test 'a self-hosted CoinGecko pull re-checks the bots too' do
    MarketDataSettings.stubs(:deltabadger?).returns(false)
    MarketData.stubs(:coingecko).returns(stub(get_categories_with_market_data: Result::Success.new([])))
    bot = index_bot(:scheduled)

    Bot::ResyncIndexCompositionJob.expects(:perform_later).with(bot)

    Index::SyncFromCoingeckoJob.perform_now
  end

  private

  def index_bot(status)
    create(:dca_index, exchange: @exchange, quote_asset: @quote).tap { |bot| bot.update_columns(status: Bot.statuses[status]) }
  end
end
