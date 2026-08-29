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
    assert_not Bots.const_defined?(:DcaDualAsset, false), 'the pair class is retired; every test in this file already runs without it'
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

    assert_equal [], Bot::DualToComposition.convert!(Bot::DualToComposition::Row.find(@bot.id))
    assert_in_delta 0.25, Bot.find(@bot.id).allocations[@base0.id.to_s], 0.000001
  end

  test 'a stale write that restores the pair shape is detected and re-converted' do
    run!
    # Exactly what a request holding a pre-flip instance does on save: settings by id, no type, and
    # the WHOLE settings hash — no allocations object. A merge that kept allocations would be the
    # harmless wizard-cookie case (see clobbered), not this.
    Bot::DualToComposition::Row.where(id: @bot.id).update_all(
      settings: Bot.find(@bot.id).settings.merge('base0_asset_id' => @base0.id,
                                                 'base1_asset_id' => @base1.id,
                                                 'allocation0' => 0.7).except('allocations')
    )

    run!
    bot = Bot.find(@bot.id)

    assert_equal 'Bots::DcaMultiAsset', bot.type
    assert_not bot.settings.key?('base0_asset_id')
    assert_in_delta 0.7, bot.allocations[@base0.id.to_s], 0.000001
  end

  # == The forced pass the retirement migration runs ==

  def finalize! = Bot::DualToComposition.finalize!

  test 'the forced pass converts a bot the normal pass leaves alone' do
    @bot.update_columns(status: Bot.statuses[:executing])
    enqueue_job_for(@bot, state: :scheduled, scheduled_at: 1.minute.ago) # due, so busy_job? is true

    _, skipped = run!
    assert_includes skipped.map(&:last), 'executing'

    converted, degraded, failed = finalize!

    assert_equal [@bot.id], converted
    assert_empty degraded
    assert_empty failed
    assert_equal 'Bots::DcaMultiAsset', Bot.find(@bot.id).type
    # Nothing ever revives an executing row (ActionJob returns on line 1, the orphan repair skips
    # it); retrying is what a working bot whose tick did not finish is meant to be.
    assert_equal 'retrying', Bot.find(@bot.id).status
  end

  test 'a scheduled bot keeps its status on the forced pass' do
    @bot.update_columns(status: Bot.statuses[:scheduled])

    finalize!

    assert_equal 'scheduled', Bot.find(@bot.id).status
  end

  test 'a gap switches rebalancing off, so the stopped bot is not a rebalance candidate' do
    # EvaluateRebalancersJob takes stopped bots with the switch on, and rebalance! refreshes the
    # composition before it looks at anything — a 50/50 fallback would be traded toward.
    write_settings!('allocation0' => 'garbage', 'rebalance_enabled' => true)
    @bot.update_columns(status: Bot.statuses[:scheduled])

    finalize!

    bot = Bot.find(@bot.id)
    assert_equal 'stopped', bot.status
    assert_not bot.rebalance_enabled?
    assert_not Bot::EvaluateRebalancersJob.new.send(:candidates).exists?(id: @bot.id)
  end

  test 'a disabled condition pointing outside the basket is not a gap' do
    # Each concern's set_*_in_ticker_id callback writes a default subject whether or not the
    # condition is on; only an ENABLED condition's subject has to be a member.
    outsider = create(:ticker, exchange_id: @bot.exchange_id,
                               base_asset: create(:asset, symbol: 'SOL', name: 'Solana'),
                               quote_asset_id: @bot.settings['quote_asset_id'])
    write_settings!('price_limit_in_ticker_id' => outsider.id, 'price_limited' => false)

    converted, skipped = run!

    assert_equal [@bot.id], converted
    assert_empty skipped
  end

  test 'the forced pass repoints a job that was already due' do
    job = enqueue_job_for(@bot, state: :scheduled, scheduled_at: 1.minute.ago)

    finalize!

    assert_includes gid_in(job), 'Bots::DcaMultiAsset'
  end

  test 'a rebalance in flight is carried over verbatim' do
    pending = { 'phase' => 'selling', 'sell_transaction_id' => 42 }
    @bot.update_columns(transient_data: @bot.transient_data.merge('rebalance_pending' => pending))

    finalize!

    assert_equal pending, Bot.find(@bot.id).transient_data['rebalance_pending']
  end

  test 'a delisted member keeps its seat, and a working bot is stopped rather than renormalised' do
    # derive_composition drops an unavailable member and renormalises the rest: left running, this
    # 70/30 bot would trade as 100/0 on its next tick.
    ticker = Ticker.find_by!(exchange_id: @bot.exchange_id, base_asset_id: @base1.id)
    ticker.update_columns(available: false)
    @bot.update_columns(status: Bot.statuses[:scheduled])

    _, skipped = run!
    assert_includes skipped.map(&:last), 'missing ticker'

    converted, degraded, = finalize!

    assert_empty converted
    assert_equal [[@bot.id, ['delisted member']]], degraded
    bot = Bot.find(@bot.id)
    assert_equal 'stopped', bot.status
    assert_equal ticker.id, BotIndexAsset.find_by!(bot_id: @bot.id, asset_id: @base1.id).ticker_id
    assert_in_delta 0.3, bot.settings['allocations'][@base1.id.to_s], 0.000001, 'the weight the user chose is still on record'
  end

  test 'a gap turns a rebalance in flight into the ambiguous halt' do
    # EvaluateRebalancersJob evaluates stopped bots too; a pending sell→buy must not resume against
    # a composition that just lost a member.
    Ticker.where(exchange_id: @bot.exchange_id, base_asset_id: @base1.id).delete_all
    pending = { 'phase' => 'selling', 'sell_transaction_id' => 42 }
    @bot.update_columns(transient_data: @bot.transient_data.merge('rebalance_pending' => pending))

    finalize!

    bot = Bot.find(@bot.id)
    assert bot.rebalance_ambiguous?
    assert_equal 42, bot.rebalance_pending[:sell_transaction_id], 'the rest of the payload is kept for the user to resolve'
  end

  test 'a queue failure after the flip is reported, and does not stop the bot' do
    @bot.update_columns(status: Bot.statuses[:scheduled])
    Bot::DualToComposition.stubs(:repoint_every_job!).raises(ActiveRecord::StatementInvalid, 'database is locked')

    converted, _, failed = finalize!

    assert_equal [@bot.id], converted
    assert_equal ['queue'], failed.map(&:first)
    assert_equal 'scheduled', Bot.find(@bot.id).status
  end

  test 'a member with no ticker row at all is dropped, and a working bot is stopped' do
    Ticker.where(exchange_id: @bot.exchange_id, base_asset_id: @base1.id).delete_all
    @bot.update_columns(status: Bot.statuses[:scheduled])

    converted, degraded, failed = finalize!

    assert_empty converted
    assert_empty failed
    assert_equal [[@bot.id, ['missing ticker']]], degraded
    bot = Bot.find(@bot.id)
    assert_equal 'stopped', bot.status
    assert_equal [@base0.id], BotIndexAsset.where(bot_id: @bot.id).pluck(:asset_id)
    assert_equal 0.7, bot.settings['allocations'][@base0.id.to_s], 'the weights still record what the user chose'
  end

  test 'a bot that was not working is not stopped by a gap' do
    Ticker.where(exchange_id: @bot.exchange_id, base_asset_id: @base1.id).delete_all
    @bot.update_columns(status: Bot.statuses[:deleted])

    finalize!

    assert_equal 'deleted', Bot.find(@bot.id).status
  end

  test 'an unusable weight falls back to half and stops the bot' do
    write_settings!('allocation0' => 'garbage')
    @bot.update_columns(status: Bot.statuses[:waiting])

    _, degraded, = finalize!

    assert_equal [[@bot.id, ['allocation unusable']]], degraded
    bot = Bot.find(@bot.id)
    assert_equal 'stopped', bot.status
    assert_equal 0.5, bot.settings['allocations'][@base0.id.to_s]
    assert_equal 0.5, bot.settings['allocations'][@base1.id.to_s]
  end

  test 'a condition watching a ticker outside the basket is a gap' do
    # The basket looks a condition's subject up in its own tickers, so such a condition would never
    # be met — a "while below X, pause" bot would pause forever. Kept on record, bot stopped.
    outsider = create(:ticker, exchange_id: @bot.exchange_id,
                               base_asset: create(:asset, symbol: 'SOL', name: 'Solana'),
                               quote_asset_id: @bot.settings['quote_asset_id'])
    write_settings!('price_limit_in_ticker_id' => outsider.id, 'price_limited' => true)
    @bot.update_columns(status: Bot.statuses[:waiting])

    _, skipped = run!
    assert_includes skipped.map(&:last), 'condition watches a foreign ticker'

    _, degraded, = finalize!

    assert_equal [[@bot.id, ['condition subject outside the basket']]], degraded
    bot = Bot.find(@bot.id)
    assert_equal 'stopped', bot.status
    assert_equal outsider.id, bot.settings['price_limit_in_ticker_id']
  end

  test 'a stray membership row is exited rather than refused' do
    stray_asset = create(:asset, symbol: 'SOL', name: 'Solana')
    stray_ticker = create(:ticker, exchange_id: @bot.exchange_id, base_asset: stray_asset,
                                   quote_asset_id: @bot.settings['quote_asset_id'])
    # insert!, not create!: a membership's required `bot` would load the pair row through STI.
    BotIndexAsset.insert!({ bot_id: @bot.id, asset_id: stray_asset.id, ticker_id: stray_ticker.id,
                            in_index: true, target_allocation: 0.2 })

    _, skipped = run!
    assert_includes skipped.map(&:last), 'unexpected composition rows'

    converted, = finalize!

    assert_equal [@bot.id], converted
    stray = BotIndexAsset.find_by!(bot_id: @bot.id, asset_id: stray_asset.id)
    assert_not stray.in_index
    assert stray.exited_at.present?
    assert_equal 2, BotIndexAsset.in_index.where(bot_id: @bot.id).count
  end

  test 'a row that raises is still made loadable, and reported' do
    BotIndexAsset.any_instance.stubs(:save!).raises(ActiveRecord::RecordInvalid)
    @bot.update_columns(status: Bot.statuses[:scheduled])

    converted, _, failed = finalize!

    assert_empty converted
    assert_equal [@bot.id], failed.map(&:first)
    assert_equal 'Bots::DcaMultiAsset', Bot.where(id: @bot.id).pick(:type)
    assert_equal 'stopped', Bot.find(@bot.id).status
    assert_nothing_raised { Bot.find(@bot.id) }
  end

  test 'a row that raises still has its rebalancing switched off and its swap halted' do
    # The rolled-back transaction takes the gap safeguards with it; the fallback must write them
    # again, or a stopped bot stays a rebalance candidate with a pending swap it could resume.
    Ticker.where(exchange_id: @bot.exchange_id, base_asset_id: @base1.id).delete_all
    write_settings!('rebalance_enabled' => true)
    @bot.update_columns(status: Bot.statuses[:scheduled],
                        transient_data: @bot.transient_data.merge('rebalance_pending' => { 'phase' => 'selling', 'sell_transaction_id' => 42 }))
    BotIndexAsset.any_instance.stubs(:save!).raises(RuntimeError, 'boom')

    finalize!

    bot = Bot.find(@bot.id)
    assert_equal 'stopped', bot.status
    assert_not bot.rebalance_enabled?
    assert bot.rebalance_ambiguous?
    # Still a candidate — candidates take any pending row — so the halt itself is what must hold.
    bot.exchange.expects(:market_sell).never
    bot.exchange.expects(:market_buy).never
    assert_equal :ambiguous, bot.rebalance!.data[:skipped]
  end

  test 'the halted swap keeps the rebalancer off the composition, not just off the venue' do
    # rebalance! used to refresh the composition BEFORE looking at the pending state; on a bot that
    # just lost a member that refresh renormalised the survivor to 100% while the halt did nothing.
    Ticker.where(exchange_id: @bot.exchange_id, base_asset_id: @base1.id).delete_all
    @bot.update_columns(transient_data: @bot.transient_data.merge('rebalance_pending' => { 'phase' => 'selling', 'sell_transaction_id' => 42 }))

    finalize!
    bot = Bot.find(@bot.id)
    targets_before = BotIndexAsset.where(bot_id: bot.id).order(:asset_id).pluck(:asset_id, :target_allocation, :in_index)
    floats = targets_before.map { |id, t, i| [id, t.to_f, i] }
    assert_equal [[@base0.id, 0.7, true]], floats

    result = bot.rebalance!

    assert_equal :ambiguous, result.data[:skipped]
    assert_equal targets_before, BotIndexAsset.where(bot_id: bot.id).order(:asset_id).pluck(:asset_id, :target_allocation, :in_index)
  end

  test 'a basket that merely carries a stale pair key is not clobbered' do
    # A pre-#229 wizard cookie can leave base0_asset_id on a good basket. Re-converting it would
    # overwrite the user's allocations with the cookie's and exit the third member.
    sol = create(:asset, symbol: 'SOL', name: 'Solana')
    basket = create(:dca_multi_asset, exchange: Exchange.find(@bot.exchange_id),
                                      base_assets: [@base0, @base1, sol],
                                      quote_asset: Asset.find(@bot.settings['quote_asset_id']),
                                      allocations: { @base0 => 0.5, @base1 => 0.3, sol => 0.2 })
    basket.update_columns(settings: basket.settings.merge('base0_asset_id' => @base0.id, 'base1_asset_id' => @base1.id, 'allocation0' => 0.9))

    assert_not Bot::DualToComposition.clobbered.exists?(id: basket.id)
    finalize!

    basket.reload
    assert_in_delta 0.2, basket.allocations[sol.id.to_s], 0.000001
    assert_equal 3, BotIndexAsset.in_index.where(bot_id: basket.id).count
  end

  test 'a basket a stale pair instance wrote the pair shape onto is converted too' do
    run!
    Bot.find(@bot.id) # a basket now
    clobber = @bot.reload.settings.merge('base0_asset_id' => @base0.id, 'base1_asset_id' => @base1.id,
                                         'allocation0' => 0.7).except('allocations')
    Bot::DualToComposition::Row.find(@bot.id).update_columns(settings: clobber)
    assert Bot::DualToComposition.clobbered.exists?(id: @bot.id)

    converted, = finalize!

    assert_equal [@bot.id], converted
    assert_not Bot::DualToComposition.clobbered.exists?(id: @bot.id)
    assert_in_delta 0.7, Bot.find(@bot.id).settings['allocations'][@base0.id.to_s], 0.000001
  end

  test 'a salvage that itself raises does not stop the pass' do
    other = pair_row(status: :waiting, exchange: Exchange.find(@bot.exchange_id),
                     base_assets: [@base0, @base1], quote_asset: Asset.find(@bot.settings['quote_asset_id']))
    BotIndexAsset.any_instance.stubs(:save!).raises(RuntimeError, 'boom')
    Bot::DualToComposition::Row.any_instance.stubs(:update_columns).raises(ActiveRecord::StatementInvalid, 'disk I/O error')

    _, _, failed = nil
    assert_nothing_raised { _, _, failed = finalize! }

    assert_equal [@bot.id, other.id].sort, failed.map(&:first).sort
  end

  test 'a row whose settings are not even a hash is made loadable' do
    @bot.update_columns(settings: [1, 2, 3])
    @bot.update_columns(status: Bot.statuses[:scheduled])

    _, _, failed = finalize!

    assert_equal [@bot.id], failed.map(&:first)
    bot = Bot.find(@bot.id)
    assert_equal({}, bot.settings.slice('base0_asset_id', 'allocations'))
    assert_equal 'stopped', bot.status
  end

  test 'the forced pass never raises out of the loop' do
    other = pair_row(status: :waiting, exchange: Exchange.find(@bot.exchange_id),
                     base_assets: [@base0, @base1], quote_asset: Asset.find(@bot.settings['quote_asset_id']))
    BotIndexAsset.any_instance.stubs(:save!).raises(RuntimeError, 'boom')

    assert_nothing_raised { finalize! }
    assert_equal %w[Bots::DcaMultiAsset Bots::DcaMultiAsset],
                 Bot.where(id: [@bot.id, other.id]).pluck(:type)
  end

  test 'a settings cell that is not JSON does not stop the pass' do
    # clobbered's WHERE filters on type = MULTI before it ever touches settings, so SQLite
    # short-circuits past a pair row's malformed cell without evaluating json_extract on it — only a
    # basket's own malformed cell reaches the function and raises. Corrupt a basket here to actually
    # exercise the rescue, and check the pair row converts regardless: it is reachable by type alone.
    corrupt = create(:dca_multi_asset, exchange: Exchange.find(@bot.exchange_id),
                                       base_assets: [@base0, @base1], quote_asset: Asset.find(@bot.settings['quote_asset_id']))
    Bot::DualToComposition::Row.connection.execute("UPDATE bots SET settings = 'not json' WHERE id = #{corrupt.id}")

    failed = nil
    assert_nothing_raised { _, _, failed = finalize! }

    assert_includes failed.map(&:first), 'scan'
    assert_equal 'Bots::DcaMultiAsset', Bot.where(id: @bot.id).pick(:type)
  end

  # == The retirement migration ==

  test 'the retirement migration converts what is left and never raises' do
    require Rails.root.glob('db/migrate/*_retire_pair_bots.rb').sole
    BotIndexAsset.any_instance.stubs(:save!).raises(RuntimeError, 'boom')

    assert_nothing_raised { capture_io { RetirePairBots.new.up } }

    assert_equal 'Bots::DcaMultiAsset', Bot.where(id: @bot.id).pick(:type)
  end
end
