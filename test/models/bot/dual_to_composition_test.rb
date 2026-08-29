require 'test_helper'

# The in-place conversion of a pair bot into a two-asset basket. What matters here is what SURVIVES:
# the carry, the schedule anchor, the trading conditions, the transaction history, and the queued
# jobs — whose GlobalIDs embed the class name and so have to be repointed rather than dropped.
class Bot::DualToCompositionTest < ActiveSupport::TestCase
  setup do
    # Transaction's after_commit enqueues Bot::UpdateMetricsJob (transaction.rb:14-16), landing a job
    # that carries the bot's old GlobalID in ReadyExecution — which busy_job? then reads as "job in
    # flight". Stubbed here, not inside run!: the fixtures enqueue it before run! is ever reached.
    Bot::UpdateMetricsJob.stubs(:perform_later)
    @bot = pair_row(status: :waiting, allocation0: 0.7)
    @base0 = Asset.find(@bot.settings['base0_asset_id'])
    @base1 = Asset.find(@bot.settings['base1_asset_id'])
  end

  def run! = Bot::DualToComposition.run!

  # A pair row without the pair class, which the retirement release deletes: a basket from the
  # factory (user, exchange, tickers, api key), re-typed and re-shaped in place into exactly the
  # rows the converter meets in the wild. Returned as the converter's own Row, because Bot.find of
  # a row whose class is gone raises — and that is the point of these tests.
  # Pass exchange:/base_assets:/quote_asset: to build a second row in the same test — the factory's
  # defaults create a Binance exchange and BTC/ETH/USD assets whose uniqueness constraints refuse a
  # second copy.
  def pair_row(status: :waiting, allocation0: 0.5, settings: {}, exchange: nil, base_assets: nil, quote_asset: nil)
    basket = create(:dca_multi_asset, status: status, exchange: exchange, base_assets: base_assets,
                                      quote_asset: quote_asset)
    base0, base1 = basket.base_asset_ids
    BotIndexAsset.where(bot_id: basket.id).delete_all
    pair_settings = basket.settings.except('allocations', 'weighting')
                          .merge('base0_asset_id' => base0, 'base1_asset_id' => base1, 'allocation0' => allocation0)
                          .merge(settings)
    Bot::DualToComposition::Row.find(basket.id).tap do |row|
      row.update_columns(type: 'Bots::DcaDualAsset', settings: pair_settings)
    end
  end

  # update_columns skips callbacks entirely, which is what a fixture wants (Bot::Accountable would
  # otherwise demand set_missed_quote_amount before any settings save).
  def write_settings!(**pairs)
    @bot.update_columns(settings: @bot.settings.merge(pairs))
    @bot.reload
  end

  # An order on a pair row, written without loading the row: Transaction validates its bot, and
  # once the pair class is gone that load raises.
  def order_on(row, **columns)
    Transaction.insert!({ bot_id: row.id, exchange_id: row.exchange_id,
                          status: Transaction.statuses[:submitted],
                          external_status: Transaction.external_statuses[:closed],
                          side: Transaction.sides[:buy], order_type: Transaction.order_types[:market_order],
                          transaction_type: 'REGULAR', base: @base0.symbol, quote: 'USD',
                          external_id: "o-#{SecureRandom.hex(4)}",
                          amount: 1, price: 100, quote_amount: 100 }.merge(columns))
    Transaction.where(bot_id: row.id).order(:id).last
  end

  # A queued job addressed to the row under the OLD class name, in a given execution state. Built by
  # hand because ActiveJob would serialize the row's real class. Solid Queue's after_create places
  # it in Ready, which is then moved to model the other states — the pattern
  # test/models/bot/limit_checkable_test.rb:80-96 uses. :none leaves no execution at all, which is
  # what the GlobalID-repointing tests want.
  def enqueue_job_for(row, state: :none, scheduled_at: 1.hour.from_now)
    job = SolidQueue::Job.create!(
      queue_name: 'default', class_name: 'Bot::ActionJob', priority: 0,
      arguments: { 'job_class' => 'Bot::ActionJob',
                   'arguments' => [{ '_aj_globalid' => "gid://deltabadger/Bots::DcaDualAsset/#{row.id}" }] }
    )
    SolidQueue::ReadyExecution.where(job_id: job.id).delete_all unless state == :ready

    case state
    when :claimed
      process = SolidQueue::Process.create!(kind: 'Worker', pid: 1, name: "worker-#{job.id}",
                                            last_heartbeat_at: Time.current)
      SolidQueue::ClaimedExecution.create!(job_id: job.id, process_id: process.id)
    when :blocked
      job.update!(concurrency_key: "bot_#{row.id}")
      SolidQueue::BlockedExecution.create!(job_id: job.id, queue_name: job.queue_name,
                                           priority: job.priority, concurrency_key: job.concurrency_key,
                                           expires_at: 5.minutes.from_now)
    when :scheduled
      job.update!(scheduled_at: scheduled_at)
      SolidQueue::ScheduledExecution.create!(job_id: job.id, queue_name: job.queue_name,
                                             priority: job.priority, scheduled_at: scheduled_at)
    end
    job
  end

  def gid_in(job) = job.reload.arguments['arguments'].first['_aj_globalid']

  # == Surviving the class's retirement ==

  # The retirement release deletes Bots::DcaDualAsset and then runs this same conversion from a
  # migration. A membership's required `bot` is loaded through STI to validate it, so the row has
  # to be a basket BEFORE the first membership is written, or every remaining row raises.
  test 'a pair row converts when its class no longer exists' do
    skip 'the class is already gone' unless Bots.const_defined?(:DcaDualAsset, false)

    # Reference the class before removing it: with eager_load off, remove_const on a constant
    # Zeitwerk has not autoloaded yet returns nil, and the restore below would then poison every
    # later test in the process.
    klass = Bots::DcaDualAsset
    Bots.send(:remove_const, :DcaDualAsset)
    begin
      assert_raises(ActiveRecord::SubclassNotFound) { Bot.find(@bot.id) }

      converted, skipped = run!

      assert_equal [@bot.id], converted
      assert_empty skipped
      assert_equal 'Bots::DcaMultiAsset', Bot.find(@bot.id).type
      assert_equal 2, BotIndexAsset.where(bot_id: @bot.id).count
    ensure
      Bots.const_set(:DcaDualAsset, klass)
    end
  end

  # == What the conversion produces ==

  test 'the bot becomes a multi-asset bot' do
    run!

    assert_equal 'Bots::DcaMultiAsset', Bot.find(@bot.id).type
  end

  test 'the pair allocation becomes the basket weights, unrounded' do
    write_settings!('allocation0' => 0.6667)
    run!
    bot = Bot.find(@bot.id)

    assert_in_delta 0.6667, bot.allocations[@base0.id.to_s], 0.000001
    assert_in_delta 0.3333, bot.allocations[@base1.id.to_s], 0.000001
    assert_not bot.settings.key?('allocation0')
    assert_not bot.settings.key?('base0_asset_id')
  end

  test 'exactly two composition rows are created' do
    run!
    bot = Bot.find(@bot.id)

    assert_equal [@base0.id, @base1.id].sort, bot.bot_index_assets.in_index.pluck(:asset_id).sort
    assert_in_delta 0.7, bot.bot_index_assets.find_by(asset_id: @base0.id).target_allocation, 0.000001
  end

  test 'a market-cap pair becomes a market-cap basket with derived membership, not frozen weights' do
    @base0.update!(market_cap: 750.0)
    @base1.update!(market_cap: 250.0)
    write_settings!('marketcap_allocated' => true)
    run!
    bot = Bot.find(@bot.id)

    assert_equal 'market_cap', bot.weighting
    assert_in_delta 0.75, bot.bot_index_assets.find_by(asset_id: @base0.id).target_allocation, 0.000001
  end

  # == What survives ==

  test 'trading conditions survive the move' do
    ticker = Ticker.find_by!(exchange_id: @bot.exchange_id, base_asset_id: @base0.id,
                             quote_asset_id: @bot.settings['quote_asset_id'])
    write_settings!('price_limited' => true, 'price_limit' => 50_000.0,
                    'price_limit_in_ticker_id' => ticker.id)
    run!
    bot = Bot.find(@bot.id)

    assert bot.price_limited?
    assert_equal 50_000.0, bot.price_limit
    assert_equal ticker.id, bot.price_limit_in_ticker_id
  end

  test 'the carry and the schedule anchor are preserved' do
    # missed_quote_amount is a transient_data accessor (accountable.rb:5), not a settings key, and
    # conversion never writes transient_data — this guards that staying true.
    anchor = 3.days.ago.change(usec: 0)
    @bot.update_columns(started_at: anchor,
                        transient_data: @bot.transient_data.merge('missed_quote_amount' => 42.0))
    run!
    bot = Bot.find(@bot.id)

    assert_equal anchor.to_i, bot.started_at.to_i
    assert_equal 42.0, bot.missed_quote_amount.to_f
  end

  test 'transactions are not touched' do
    order_on(@bot, base: 'BTC', quote: 'USD',
                   status: Transaction.statuses[:submitted], external_status: Transaction.external_statuses[:closed])
    before = Transaction.where(bot_id: @bot.id).pluck(:id, :base, :quote, :amount_exec).sort
    run!

    assert_equal before, Transaction.where(bot_id: @bot.id).pluck(:id, :base, :quote, :amount_exec).sort
  end

  # == Queued jobs ==

  test 'a queued job is repointed at the new class rather than destroyed' do
    job = enqueue_job_for(@bot)
    run!

    assert_equal Bot.find(@bot.id).to_global_id.to_s, gid_in(job)
  end

  test 'the repointed job still resolves to the same bot' do
    job = enqueue_job_for(@bot)
    run!

    assert_equal @bot.id, GlobalID::Locator.locate(gid_in(job)).id
  end

  test "another bot's queued job is left alone" do
    # Same exchange (Exchange#type is unique) and its own assets, so it is a distinct bot that the
    # conversion will skip — it is :executing — while still owning a queued job.
    other = pair_row(status: :executing, exchange: Exchange.find(@bot.exchange_id),
                     base_assets: [create(:asset, symbol: 'SOL', name: 'Solana'),
                                   create(:asset, symbol: 'ADA', name: 'Cardano')],
                     quote_asset: Asset.find(@bot.settings['quote_asset_id']))
    job = enqueue_job_for(other)
    run!

    assert_equal "gid://deltabadger/Bots::DcaDualAsset/#{other.id}", gid_in(job)
  end

  test 'a later pass repoints a stray job a worker enqueued after the flip' do
    run!
    stray = SolidQueue::Job.create!(
      queue_name: 'default', class_name: 'Bot::ActionJob', priority: 0,
      arguments: { 'arguments' => [{ '_aj_globalid' => "gid://deltabadger/Bots::DcaDualAsset/#{@bot.id}" }] }
    )

    run!

    assert_includes stray.reload.arguments['arguments'].first['_aj_globalid'], 'Bots::DcaMultiAsset'
  end

  # == What is refused ==

  test 'an executing bot is left alone' do
    @bot.update_columns(status: Bot.statuses[:executing])
    _, skipped = run!

    assert_equal 'Bots::DcaDualAsset', Bot.where(id: @bot.id).pick(:type)
    assert_includes skipped.map(&:last), 'executing'
  end

  test 'a bot with a rebalance in flight is left alone' do
    @bot.update_columns(transient_data: @bot.transient_data.merge('rebalance_pending' => { 'phase' => 'sold' }))
    _, skipped = run!

    assert_equal 'Bots::DcaDualAsset', Bot.where(id: @bot.id).pick(:type)
    assert_includes skipped.map(&:last), 'rebalance in flight'
  end

  # A standing limit order is a limit bot's resting state, not money in flight: it is polled by
  # bot_id at the start of every cycle (Bot::LimitOrderable), so it carries over the flip untouched
  # and the basket picks it up. Refusing it would never convert a limit-discount bot at all.
  test 'a bot with an order still open on a venue converts and keeps the order' do
    order = order_on(@bot, external_status: Transaction.external_statuses[:open])
    _, skipped = run!

    assert_empty skipped
    assert_equal 'Bots::DcaMultiAsset', Bot.find(@bot.id).type
    assert Bot.find(@bot.id).transactions.waiting.exists?(order.id)
    assert_equal %w[submitted open], order.reload.values_at(:status, :external_status)
  end

  test 'a settled order does not block conversion' do
    order_on(@bot)
    run!

    assert_equal 'Bots::DcaMultiAsset', Bot.find(@bot.id).type
  end

  %i[claimed ready blocked].each do |state|
    test "a bot whose action job is #{state} is left alone" do
      enqueue_job_for(@bot, state: state)
      _, skipped = run!

      assert_equal 'Bots::DcaDualAsset', Bot.where(id: @bot.id).pick(:type)
      assert_includes skipped.map(&:last), 'job in flight'
    end
  end

  test 'a scheduled job that is already due is left alone' do
    enqueue_job_for(@bot, state: :scheduled, scheduled_at: 1.minute.ago)
    _, skipped = run!

    assert_equal 'Bots::DcaDualAsset', Bot.where(id: @bot.id).pick(:type)
    assert_includes skipped.map(&:last), 'job in flight'
  end

  test 'a job scheduled in the future does not block conversion' do
    enqueue_job_for(@bot, state: :scheduled, scheduled_at: 1.hour.from_now)
    run!

    assert_equal 'Bots::DcaMultiAsset', Bot.find(@bot.id).type
  end

  test 'a non-numeric allocation is refused rather than read as zero' do
    write_settings!('allocation0' => 'garbage')
    _, skipped = run!

    assert_equal 'Bots::DcaDualAsset', Bot.where(id: @bot.id).pick(:type)
    assert_includes skipped.map(&:last), 'allocation unusable'
  end

  test 'a ticker the target class would refuse to derive from blocks conversion' do
    Ticker.find_by(exchange: Exchange.find(@bot.exchange_id), base_asset_id: @base1.id)
          .update!(trading_enabled: false)
    _, skipped = run!

    assert_equal 'Bots::DcaDualAsset', Bot.where(id: @bot.id).pick(:type)
    assert_includes skipped.map(&:last), 'missing ticker'
    assert_equal 0, BotIndexAsset.where(bot_id: @bot.id).count
  end

  test 'an unexpected composition row blocks conversion rather than adding a member' do
    stray = create(:asset, symbol: 'SOL', name: 'Solana')
    BotIndexAsset.insert!({ bot_id: @bot.id, asset_id: stray.id, ticker_id: Ticker.first.id,
                            in_index: true, target_allocation: 0.1 })
    _, skipped = run!

    assert_equal 'Bots::DcaDualAsset', Bot.where(id: @bot.id).pick(:type)
    assert_includes skipped.map(&:last), 'unexpected composition rows'
  end

  # == Recovery ==

  test 'running it twice changes nothing the second time' do
    run!
    snapshot = Bot.find(@bot.id).attributes
    rows = BotIndexAsset.where(bot_id: @bot.id).count
    run!

    assert_equal snapshot, Bot.find(@bot.id).attributes
    assert_equal rows, BotIndexAsset.where(bot_id: @bot.id).count
  end

  test 'a conversion interrupted before its queue write is finished by a re-run' do
    job = enqueue_job_for(@bot)
    Bot::DualToComposition::Row.where(id: @bot.id).update_all(type: 'Bots::DcaMultiAsset')

    run!

    assert_includes gid_in(job), 'Bots::DcaMultiAsset'
  end

  test 'a clobbered row that cannot be re-converted keeps its basket type' do
    # Flipping the type back before checking would leave a bot typed as a pair while its
    # memberships and queued GlobalIDs were already basket-shaped — worse than the clobber.
    run!
    Bot::DualToComposition::Row.where(id: @bot.id).update_all(
      settings: Bot.find(@bot.id).settings.merge('base0_asset_id' => @base0.id,
                                                 'base1_asset_id' => @base1.id,
                                                 'allocation0' => 0.7),
      status: Bot.statuses[:executing]
    )

    run!

    assert_equal 'Bots::DcaMultiAsset', Bot::DualToComposition::Row.find(@bot.id).type
  end

  test 'a settings edit landing between preflight and the lock is not overwritten' do
    # preflight runs before the lock, so its plan can describe weights a request has already
    # replaced. The in-lock re-run is what stops that edit being silently discarded.
    row = Bot::DualToComposition::Row.find(@bot.id)
    stale_plan = Bot::DualToComposition.preflight(row)
    assert_in_delta 0.7, stale_plan[:allocation0], 0.000001

    write_settings!('allocation0' => 0.25) # stands in for the concurrent request

    assert_equal true, Bot::DualToComposition.convert!(Bot::DualToComposition::Row.find(@bot.id))
    assert_in_delta 0.25, Bot.find(@bot.id).allocations[@base0.id.to_s], 0.000001
  end

  test 'a stale write that restores the pair shape is detected and re-converted' do
    run!
    # Exactly what a request holding a pre-flip instance does on save: settings by id, no type.
    Bot::DualToComposition::Row.where(id: @bot.id).update_all(
      settings: Bot.find(@bot.id).settings.merge('base0_asset_id' => @base0.id,
                                                 'base1_asset_id' => @base1.id,
                                                 'allocation0' => 0.7)
    )

    run!
    bot = Bot.find(@bot.id)

    assert_equal 'Bots::DcaMultiAsset', bot.type
    assert_not bot.settings.key?('base0_asset_id')
    assert_in_delta 0.7, bot.allocations[@base0.id.to_s], 0.000001
  end
end
