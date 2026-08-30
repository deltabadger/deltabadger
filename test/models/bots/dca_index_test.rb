require 'test_helper'
require 'turbo/broadcastable/test_helper'

class Bots::DcaIndexTest < ActiveSupport::TestCase
  include ExchangeMockHelpers
  include Turbo::Broadcastable::TestHelper
  include ActiveJob::TestHelper

  # The app pins the SolidQueue adapter, and ActiveJob::TestHelper leaves a configured adapter
  # alone — so ask for the test one explicitly, or perform_enqueued_jobs has nothing to perform.
  def queue_adapter_for_test
    ActiveJob::QueueAdapters::TestAdapter.new
  end

  setup do
    @exchange = create(:kraken_exchange)
    @quote = create(:asset, :eur)

    # Candidates in CoinGecko rank order. "coin-dead" sits between two live
    # coins so that filtering it must backfill from a lower-ranked candidate.
    @asset_a, @ticker_a = create_candidate('coin-a', 'AAA')
    @asset_dead, @ticker_dead = create_candidate('coin-dead', 'DEAD')
    @asset_b, @ticker_b = create_candidate('coin-b', 'BBB')
    @asset_c, @ticker_c = create_candidate('coin-c', 'CCC')

    stub_top_coins(%w[coin-a coin-dead coin-b coin-c])
  end

  test 'the composition machinery is shared and the index derives it from market data' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    bot.num_coins = 2
    stub_all_priced(:get_ask_price)

    assert_predicate bot.refresh_composition, :success?
    assert_equal 2, bot.bot_index_assets.in_index.count
    %w[Allocatable Measurable Liquidatable OrderSetter Rebalancer].each do |name|
      assert_includes Bots::DcaIndex.ancestors, "Bot::Composition::#{name}".constantize
    end
    assert_equal bot.num_coins, bot.composition_size
    assert_equal 'bot.dca_index.left_the_index', bot.exited_title_key
    assert_equal 'bots/composition/metrics', bot.metrics_partial
  end

  test 'market-order index skips a pair with no ask price and backfills from the next candidate' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    bot.num_coins = 2

    # All pairs priced on last; dead pair priced on last too — so if selection
    # wrongly probed last instead of ask, it would NOT be filtered.
    stub_all_priced(:get_last_price)
    stub_all_priced(:get_ask_price)
    stub_unpriced(:get_ask_price, @ticker_dead)

    result = bot.refresh_composition

    assert_predicate result, :success?
    assert_equal %w[coin-a coin-b], in_index_external_ids(bot)
    assert_not_includes in_index_external_ids(bot), 'coin-dead'
  end

  test 'limit-order index skips a pair with no last price and backfills from the next candidate' do
    bot = create(:dca_index, :limit_ordered, exchange: @exchange, quote_asset: @quote)
    bot.num_coins = 2

    stub_all_priced(:get_ask_price)
    stub_all_priced(:get_last_price)
    stub_unpriced(:get_last_price, @ticker_dead)

    result = bot.refresh_composition

    assert_predicate result, :success?
    assert_equal %w[coin-a coin-b], in_index_external_ids(bot)
    assert_not_includes in_index_external_ids(bot), 'coin-dead'
  end

  test 'a pair that raises on price (zero-price guard) is treated as unpriced and skipped' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    bot.num_coins = 2

    stub_all_priced(:get_ask_price)
    Exchanges::Kraken.any_instance.stubs(:get_ask_price)
                     .with(ticker: @ticker_dead, force: anything)
                     .raises(RuntimeError.new('Wrong ask price for DEADEUR: 0.0'))

    result = bot.refresh_composition

    assert_predicate result, :success?
    assert_equal %w[coin-a coin-b], in_index_external_ids(bot)
  end

  test 'returns a failure when no candidate pair is tradeable' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    bot.num_coins = 2

    stub_all_unpriced(:get_ask_price)

    result = bot.refresh_composition

    assert_predicate result, :failure?
    assert_empty bot.bot_index_assets.in_index
  end

  test 'selection excludes a trading-disabled pair and backfills (even when priced)' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    bot.num_coins = 2
    @ticker_dead.update!(trading_enabled: false)
    # Everything is priced, so only the trading_enabled filter can exclude the dead pair.
    stub_all_priced(:get_ask_price)
    stub_all_priced(:get_last_price)

    result = bot.refresh_composition

    assert_predicate result, :success?
    assert_equal %w[coin-a coin-b], in_index_external_ids(bot)
  end

  # --- an incumbent keeps its seat ---------------------------------------------
  #
  # The probe's one irreplaceable job is BACKFILL: deciding which NEW candidate fills a slot.
  # Re-probing a coin already in the index can only ever evict it, and Ticker#priced? cannot tell a
  # delisting from a proxy 502 (an HTTP failure comes back as a plain false), so a network blip was
  # demoting a held constituent to "Left the index" — where Liquidatable#liquidate_exited!, which
  # refreshes strictly before it sells, would then sell it.

  test 'an incumbent whose price probe fails keeps its seat instead of being backfilled over' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    bot.num_coins = 2
    stub_all_priced(:get_ask_price)
    bot.refresh_composition
    assert_equal %w[coin-a coin-dead], in_index_external_ids(bot)

    stub_unpriced(:get_ask_price, @ticker_dead)
    bot.refresh_composition

    assert_equal %w[coin-a coin-dead], in_index_external_ids(bot), 'a blip must not evict a constituent'
    assert_not_includes in_index_external_ids(bot), 'coin-b', 'and must not buy a replacement for it'
  end

  test 'an incumbent still exits when the catalogue drops it' do
    # The seat rests on the venue's own listing status, not on nothing.
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    bot.num_coins = 2
    stub_all_priced(:get_ask_price)
    bot.refresh_composition

    @ticker_dead.update!(trading_enabled: false)
    bot.refresh_composition

    assert_equal %w[coin-a coin-b], in_index_external_ids(bot)
  end

  test 'an incumbent is not re-probed' do
    # The canary: this is the assertion that fails if the seat is ever taken away again.
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    bot.num_coins = 2
    stub_all_priced(:get_ask_price)
    bot.refresh_composition

    Exchanges::Kraken.any_instance.expects(:get_ask_price).with(ticker: @ticker_a, force: anything).never

    bot.refresh_composition
  end

  test 'current_index_preview excludes trading-disabled pairs' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    @ticker_dead.update!(trading_enabled: false)

    symbols = bot.current_index_preview.map { |p| p[:symbol] }

    assert_not_includes symbols, 'DEAD'
    assert_includes symbols, 'AAA'
  end

  # --- the composition follows the settings ------------------------------------
  #
  # The assets table splits on the PERSISTED composition, and until now that was only re-derived at
  # the next buy or rebalance. So moving the coins slider changed the donut (drawn live from the
  # preview) and nothing else: a coin the slider had just taken back in stayed sitting under "Left
  # the index" with a Sell button over it, for up to an interval.

  test 'raising the coin count takes a dropped coin back in, without waiting for the next buy' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    stub_all_priced(:get_ask_price)
    update_settings!(bot, 'num_coins' => 2)
    assert_equal %w[coin-a coin-dead], in_index_external_ids(bot)

    update_settings!(bot, 'num_coins' => 3)

    assert_equal %w[coin-a coin-b coin-dead], in_index_external_ids(bot)
    assert_empty bot.bot_index_assets.exited, 'the coin the slider took back in is no longer a quitter'
  end

  test 'lowering the coin count marks what the slider cut as a quitter straight away' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    stub_all_priced(:get_ask_price)
    update_settings!(bot, 'num_coins' => 3)

    update_settings!(bot, 'num_coins' => 2)

    assert_equal %w[coin-a coin-dead], in_index_external_ids(bot)
    assert_equal %w[coin-b], bot.bot_index_assets.exited.includes(:asset).map { |bia| bia.asset.external_id }.sort
  end

  test 'a settings change that leaves the index definition alone re-derives nothing' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    stub_all_priced(:get_ask_price)
    update_settings!(bot, 'num_coins' => 2)

    Bot::ResyncIndexCompositionJob.expects(:perform_later).never

    update_settings!(bot, 'quote_amount' => 250.0)
  end

  test 'moving the bot to another exchange re-points the composition at that exchange tickers' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    stub_all_priced(:get_ask_price)
    update_settings!(bot, 'num_coins' => 2)

    binance = create(:binance_exchange)
    %w[coin-a coin-dead].each do |external_id|
      create(:ticker, exchange: binance, base_asset: Asset.find_by(external_id: external_id), quote_asset: @quote)
    end
    Exchanges::Binance.any_instance.stubs(:get_ask_price).returns(Result::Success.new(BigDecimal('100')))

    perform_enqueued_jobs(only: Bot::ResyncIndexCompositionJob) do
      bot.set_missed_quote_amount
      bot.update!(exchange: binance)
    end

    assert_equal %w[coin-a coin-dead], in_index_external_ids(bot)
    assert_equal [binance.id], bot.bot_index_assets.in_index.includes(:ticker).map { |bia| bia.ticker.exchange_id }.uniq
  end

  test 'an upstream failure keeps the settings and leaves the old composition standing' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    stub_all_priced(:get_ask_price)
    update_settings!(bot, 'num_coins' => 2)

    MarketData.stubs(:get_top_coins).returns(Result::Failure.new('upstream down'))
    update_settings!(bot, 'num_coins' => 3)

    assert_equal 3, bot.num_coins, 'the save is not held hostage to the refresh'
    assert_equal %w[coin-a coin-dead], in_index_external_ids(bot)
  end

  # --- Lifecycle: start/stop/delete (characterization before concern extraction) -

  test 'start schedules Bot::ActionJob immediately, sets scheduled status and logs started activity' do
    MarketData.stubs(:configured?).returns(true)
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
    Bot::ActionJob.expects(:perform_later).with(bot)

    freeze_time do
      assert_difference -> { bot.bot_activity_logs.where(event: 'started').count }, 1 do
        assert_equal true, bot.start
      end
      assert_equal 'scheduled', bot.status
      assert_equal Time.current, bot.started_at
    end
  end

  test 'start clears stop_message_key and last_action_job_at' do
    MarketData.stubs(:configured?).returns(true)
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    Bot::ActionJob.stubs(:perform_later)
    bot.update!(status: :stopped, stop_message_key: 'some_key')
    bot.last_action_job_at = Time.current

    bot.start
    assert_nil bot.stop_message_key
    assert_nil bot.last_action_job_at
  end

  test 'start returns false and schedules nothing when market data is not configured' do
    MarketData.stubs(:configured?).returns(false)
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    Bot::ActionJob.expects(:perform_later).never

    assert_equal false, bot.start
    assert bot.errors[:base].present?
  end

  test 'stop cancels scheduled action jobs, stores stop_message_key and logs stopped activity' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote,
                             status: :scheduled, started_at: Time.current)
    bot.expects(:cancel_scheduled_action_jobs)

    assert_difference -> { bot.bot_activity_logs.where(event: 'stopped').count }, 1 do
      assert_equal true, bot.stop(stop_message_key: 'manual_stop')
    end
    assert_equal 'stopped', bot.status
    assert_equal 'manual_stop', bot.stop_message_key
  end

  test 'delete sets deleted status and cancels scheduled action jobs when an exchange is present' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote,
                             status: :scheduled, started_at: Time.current)
    bot.expects(:cancel_scheduled_action_jobs)

    assert_equal true, bot.delete
    assert_equal 'deleted', bot.status
  end

  # --- Validations: unchangeable settings (characterization) ---------------------

  test 'prevents changing quote asset after transactions exist' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    create(:transaction, bot: bot, base: 'AAA', quote: 'EUR')
    other_quote = create(:asset, :usd)

    bot.set_missed_quote_amount
    bot.quote_asset_id = other_quote.id
    assert_not bot.valid?(:update)
    assert bot.errors[:quote_asset_id].present?
  end

  test 'prevents changing interval while bot is running' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote,
                             status: :scheduled, started_at: Time.current)

    bot.set_missed_quote_amount
    bot.interval = 'day'
    assert_not bot.valid?(:update)
    assert_includes bot.errors[:settings], 'Interval cannot be changed while the bot is running'
  end

  test 'prevents changing exchange when there are open orders' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    create(:transaction, :open, bot: bot, base: 'AAA', quote: 'EUR')
    new_exchange = create(:binance_exchange)
    create(:ticker, exchange: new_exchange, base_asset: @asset_a, quote_asset: @quote)

    bot.exchange = new_exchange
    assert_not bot.valid?(:update)
    assert bot.errors[:exchange].present?
  end

  test 'prevents changing the index after transactions exist' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    create(:transaction, bot: bot, base: 'AAA', quote: 'EUR')

    bot.set_missed_quote_amount
    bot.index_type = Bots::DcaIndex::INDEX_TYPE_CATEGORY
    bot.index_category_id = 'layer-1'
    assert_not bot.valid?(:update)
    assert bot.errors[:index_type].present?
  end

  # --- start: status-bar broadcast ----------------------------------------------

  test 'start with an immediate first order broadcasts the scheduled status bar' do
    MarketData.stubs(:configured?).returns(true)
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)

    streams = capture_turbo_stream_broadcasts(["user_#{bot.user_id}", :bot_updates]) do
      assert bot.start
    end

    assert streams.any? { |s| s['target'] == bot.dom_id(bot, :status_bar) },
           'an immediate start enqueues no BroadcastAfterScheduledActionJob, ' \
           'so the model itself must broadcast the "scheduled" status bar'
  end

  test 'start with a delayed first order leaves the status-bar broadcast to BroadcastAfterScheduledActionJob' do
    MarketData.stubs(:configured?).returns(true)
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    bot.settings = bot.settings.merge('start_time_enabled' => true,
                                      'start_time_mode' => 'date',
                                      'start_at' => 1.day.from_now.utc.iso8601)
    bot.set_missed_quote_amount
    bot.save!

    streams = capture_turbo_stream_broadcasts(["user_#{bot.user_id}", :bot_updates]) do
      assert bot.start
    end

    assert_not streams.any? { |s| s['target'] == bot.dom_id(bot, :status_bar) },
               'a delayed start must skip the immediate broadcast (the scheduled job handles it)'
  end

  # --- Naming (item 6) ---------------------------------------------------------

  test 'display_index_name uses index_name_prefix + num_coins when a prefix is set' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    bot.index_name_prefix = 'Nasdaq'
    bot.num_coins = 7

    assert_equal 'Nasdaq 7', bot.display_index_name
  end

  test 'display_index_name falls back to the cached category name when no prefix' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    bot.index_type = Bots::DcaIndex::INDEX_TYPE_CATEGORY
    bot.index_category_id = 'layer-1'
    bot.index_name = 'Layer 1'

    assert_equal 'Layer 1', bot.display_index_name
  end

  # --- num_coins clamp to a bounded stock index (item 6) -----------------------

  test 'num_coins is clamped to a bounded (deltabadger) index size, on validation and display' do
    Index.create!(external_id: 'nasdaq-100', source: Index::SOURCE_DELTABADGER,
                  name: 'Nasdaq 20', top_coins: (1..20).map { |i| "s#{i}" })
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    bot.index_type = Bots::DcaIndex::INDEX_TYPE_CATEGORY
    bot.index_category_id = 'nasdaq-100'
    bot.index_name_prefix = 'Nasdaq'
    bot.num_coins = 50

    bot.valid? # fires the before_validation clamp

    assert_equal 20, bot.num_coins, 'should clamp 50 down to the 20-member universe'
    assert_equal 'Nasdaq 20', bot.display_index_name, 'must not show "Nasdaq 50"'
  end

  test 'crypto Top bot is NOT clamped even when the internal index stores few coins' do
    Index.create!(external_id: Index::TOP_COINS_EXTERNAL_ID, source: Index::SOURCE_INTERNAL,
                  name: 'Top Coins', top_coins: %w[bitcoin ethereum])
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote) # index_type top
    bot.num_coins = 40

    bot.valid?

    assert_equal 40, bot.num_coins
  end

  # --- index-size default (item 6) ---------------------------------------------

  test 'a new bot on a bounded deltabadger index defaults num_coins to the full index size' do
    Index.create!(external_id: 'nasdaq-100', source: Index::SOURCE_DELTABADGER,
                  name: 'Nasdaq 20', top_coins: (1..20).map { |i| "s#{i}" })

    bot = Bots::DcaIndex.new(type: 'Bots::DcaIndex',
                             settings: { 'index_type' => Bots::DcaIndex::INDEX_TYPE_CATEGORY,
                                         'index_category_id' => 'nasdaq-100' })

    assert_equal 20, bot.num_coins
  end

  test 'a bounded index larger than MAX_COINS caps the default at MAX_COINS' do
    Index.create!(external_id: 'big-index', source: Index::SOURCE_DELTABADGER,
                  name: 'Big', top_coins: (1..60).map { |i| "s#{i}" })

    bot = Bots::DcaIndex.new(type: 'Bots::DcaIndex',
                             settings: { 'index_type' => Bots::DcaIndex::INDEX_TYPE_CATEGORY,
                                         'index_category_id' => 'big-index' })

    assert_equal Bots::DcaIndex::MAX_COINS, bot.num_coins
  end

  test 'a new crypto Top bot defaults num_coins to 10' do
    bot = Bots::DcaIndex.new(type: 'Bots::DcaIndex',
                             settings: { 'index_type' => Bots::DcaIndex::INDEX_TYPE_TOP })

    assert_equal 10, bot.num_coins
  end

  private

  def create_candidate(external_id, symbol)
    asset = create(:asset, external_id: external_id, symbol: symbol)
    ticker = create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: @quote)
    [asset, ticker]
  end

  def stub_top_coins(external_ids)
    coins = external_ids.each_with_index.map do |id, i|
      { 'id' => id, 'market_cap' => (100 - i).to_f, 'current_price' => 1.0 }
    end
    MarketData.stubs(:get_top_coins).returns(Result::Success.new(coins))
  end

  def stub_all_priced(method)
    Exchanges::Kraken.any_instance.stubs(method).returns(Result::Success.new(BigDecimal('100')))
  end

  def stub_all_unpriced(method)
    Exchanges::Kraken.any_instance.stubs(method).returns(Result::Failure.new('zero'))
  end

  def stub_unpriced(method, ticker)
    Exchanges::Kraken.any_instance.stubs(method)
                     .with(ticker: ticker, force: anything)
                     .returns(Result::Failure.new('zero'))
  end

  # What bots_controller#update does: through the missed-amount guard, and with the composition
  # resync it schedules run inline — the broadcast it in turn schedules is left enqueued.
  def update_settings!(bot, changes)
    perform_enqueued_jobs(only: Bot::ResyncIndexCompositionJob) do
      bot.set_missed_quote_amount
      bot.update!(settings: bot.settings.merge(changes))
    end
  end

  def in_index_external_ids(bot)
    bot.bot_index_assets.in_index.includes(:asset).map { |bia| bia.asset.external_id }.sort
  end
end
