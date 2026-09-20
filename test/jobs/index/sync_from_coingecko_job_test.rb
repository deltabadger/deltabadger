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

  # Drives a NON-EMPTY eligible feed on purpose: an empty one now returns before the sweeps (an
  # empty `live_ids` would make `where.not` match every row), so it would no longer reach the
  # recheck this test exists to cover.
  test 'a self-hosted CoinGecko pull re-checks the bots too' do
    coingecko_feed([category('layer-1')])
    bot = index_bot(:scheduled)

    Bot::ResyncIndexCompositionJob.expects(:perform_later).with(bot)

    Index::SyncFromCoingeckoJob.perform_now
  end

  # --- sweeps ---------------------------------------------------------------------------------
  #
  # `Index.coingecko.where.not(id: [])` compiles to `WHERE 1=1`, so the old sweep emptied the whole
  # picker whenever a run upserted nothing. Both replacements key off the ELIGIBLE ids and neither
  # may run on an empty set.

  test 'an empty categories response deletes no indices' do
    kept = create(:index, external_id: 'layer-1', source: Index::SOURCE_COINGECKO)
    coingecko_feed([])

    Index::SyncFromCoingeckoJob.perform_now

    assert Index.exists?(kept.id), 'an empty feed is a bad feed, not an instruction to delete'
  end

  test 'an index the feed no longer lists is removed' do
    gone = create(:index, external_id: 'dead-category', source: Index::SOURCE_COINGECKO)
    coingecko_feed([category('layer-1')])

    Index::SyncFromCoingeckoJob.perform_now

    assert_not Index.exists?(gone.id)
  end

  test 'a category that loses its description is removed even though the feed still lists it' do
    gone = create(:index, external_id: 'layer-1', source: Index::SOURCE_COINGECKO)
    coingecko_feed([category('layer-1').merge('content' => ''), category('layer-2')])

    Index::SyncFromCoingeckoJob.perform_now

    assert_not Index.exists?(gone.id), 'ineligible is swept by the same free, pre-fetch signal'
  end

  test 'an index fetched this run that no longer qualifies is removed' do
    gone = create(:index, external_id: 'layer-1', source: Index::SOURCE_COINGECKO)
    # Fetched, but returns too few coins we hold -> conclusively disqualified.
    coingecko_feed([category('layer-1')], coins: [])

    Index::SyncFromCoingeckoJob.perform_now

    assert_not Index.exists?(gone.id)
  end

  test 'an index whose qualification raises keeps its row' do
    kept = create(:index, external_id: 'layer-1', source: Index::SOURCE_COINGECKO)
    coingecko_feed([category('layer-1')])
    Index.stubs(:calculate_available_exchanges).raises(StandardError, 'boom')

    Index::SyncFromCoingeckoJob.perform_now

    assert Index.exists?(kept.id), 'an error is not evidence of disqualification'
  end

  test 'an index whose category was not fetched this run is kept' do
    kept = create(:index, external_id: 'layer-2', source: Index::SOURCE_COINGECKO)
    coingecko_feed([category('layer-1'), category('layer-2')], due: nil)
    Index::SyncFromCoingeckoJob.any_instance.stubs(:due_today?).with('layer-1').returns(true)
    Index::SyncFromCoingeckoJob.any_instance.stubs(:due_today?).with('layer-2').returns(false)

    Index::SyncFromCoingeckoJob.perform_now

    assert Index.exists?(kept.id)
  end

  # Availability is live ticker state and flickers with a venue feed gap; the free daily recompute
  # re-derives it. Deleting on it would hold the row out until its next slice day.
  test 'an index no venue currently offers is kept, not deleted' do
    kept = create(:index, external_id: 'layer-1', source: Index::SOURCE_COINGECKO,
                          available_exchanges: { 'Exchanges::Kraken' => 5 })
    coingecko_feed([category('layer-1')])
    Index.stubs(:calculate_available_exchanges).returns({})

    Index::SyncFromCoingeckoJob.perform_now

    assert Index.exists?(kept.id), 'it heals the day the venue lists those coins again'
    assert_empty kept.reload.available_exchanges
  end

  # --- slicing --------------------------------------------------------------------------------

  test 'only the day s slice of categories is fetched' do
    ids = 60.times.map { |i| "cat-#{i}" }
    coingecko = coingecko_feed(ids.map { |id| category(id) }, due: nil)

    Index::SyncFromCoingeckoJob.perform_now

    slice = Index::SyncFromCoingeckoJob::SLICE_DAYS
    due = ids.count { |id| Digest::MD5.hexdigest(id).to_i(16) % slice == Date.current.jd % slice }
    assert_operator due, :<, ids.size, 'the slice is a strict subset'
    assert_equal due, coingecko.fetched.size
  end

  # yday would repeat a bucket across New Year (365 % 14 != 0) and could skip one entirely.
  # jd is continuous, so a window of SLICE_DAYS consecutive dates always covers every bucket.
  test 'every category is fetched within SLICE_DAYS, including across a year boundary' do
    slice = Index::SyncFromCoingeckoJob::SLICE_DAYS
    ids = 200.times.map { |i| "cat-#{i}" }
    job = Index::SyncFromCoingeckoJob.new

    start = Date.new(2026, 12, 28)
    covered = (0...slice).flat_map do |offset|
      travel_to(start + offset) { ids.select { |id| job.send(:due_today?, id) } }
    end

    assert_equal ids.to_set, covered.to_set, 'every category refreshes within the window'
  end

  # available_exchanges gates which venues the index wizard offers, and top_coins_by_exchange gates
  # the quote picker. Slicing the FETCHES must not slice the availability recompute, which is
  # database-only and costs nothing.
  test 'availability is recomputed for every index, including ones not fetched today' do
    stale = create(:index, external_id: 'layer-2', source: Index::SOURCE_COINGECKO)
    coingecko_feed([category('layer-1'), category('layer-2')], due: false)
    Index.any_instance.expects(:refresh_available_exchanges!).at_least_once

    Index::SyncFromCoingeckoJob.perform_now

    assert Index.exists?(stale.id)
  end

  private

  def category(id)
    { 'id' => id, 'name' => id.titleize, 'content' => "About #{id}", 'market_cap' => 1_000_000.0 }
  end

  # Stubs the provider and records which categories were actually fetched.
  # `due:` pins the daily slice so a test that needs a fetch does not depend on today's date —
  # pass nil to exercise the real slice.
  def coingecko_feed(categories, coins: nil, due: true)
    MarketDataSettings.stubs(:deltabadger?).returns(false)
    Index::SyncFromCoingeckoJob.any_instance.stubs(:due_today?).returns(due) unless due.nil?
    if coins.nil?
      # Enough held assets to clear MINIMUM_SUPPORTED_COINS, so a category is only disqualified
      # when a test means it to be.
      ids = %w[bitcoin ethereum tether ripple solana]
      ids.each do |id|
        Asset.find_or_create_by!(external_id: id) do |asset|
          asset.symbol = id.upcase[0, 4]
          asset.name = id
        end
      end
      coins = ids.map { |id| { 'id' => id } }
    end
    recorder = Object.new
    recorder.define_singleton_method(:fetched) { @fetched ||= [] }
    recorder.define_singleton_method(:get_categories_with_market_data) { Result::Success.new(categories) }
    recorder.define_singleton_method(:get_coins_list_with_market_data) do |category:, **|
      fetched << category
      Result::Success.new(coins)
    end
    MarketData.stubs(:coingecko).returns(recorder)
    recorder
  end

  def index_bot(status)
    create(:dca_index, exchange: @exchange, quote_asset: @quote).tap { |bot| bot.update_columns(status: Bot.statuses[status]) }
  end
end
