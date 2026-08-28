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
    @bot = create(:dca_dual_asset, status: :waiting)
    @base0 = Asset.find(@bot.base0_asset_id)
    @base1 = Asset.find(@bot.base1_asset_id)
    write_settings!('allocation0' => 0.7)
  end

  def run! = Bot::DualToComposition.run!

  # Bot::Accountable raises on a settings save without set_missed_quote_amount (accountable.rb:82);
  # update_columns skips callbacks entirely, which is what a fixture wants.
  def write_settings!(**pairs)
    @bot.update_columns(settings: @bot.settings.merge(pairs))
    @bot.reload
  end

  # Solid Queue creates a ReadyExecution automatically on enqueue, so a hand-built Job would land in
  # Ready whatever state was asked for. Each state is modelled by MOVING that row — the pattern
  # test/models/bot/limit_checkable_test.rb:80-96 already uses. :none leaves no execution at all,
  # which is what the GlobalID-repointing tests want.
  def enqueue_job_for(bot, state: :none, scheduled_at: 1.hour.from_now)
    Bot::ActionJob.perform_later(bot)
    job = SolidQueue::Job.where(class_name: 'Bot::ActionJob').order(:id).last
    SolidQueue::ReadyExecution.where(job_id: job.id).delete_all unless state == :ready

    case state
    when :claimed
      process = SolidQueue::Process.create!(kind: 'Worker', pid: 1, name: "worker-#{job.id}",
                                            last_heartbeat_at: Time.current)
      SolidQueue::ClaimedExecution.create!(job_id: job.id, process_id: process.id)
    when :blocked
      job.update!(concurrency_key: "bot_#{bot.id}")
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
    ticker = @bot.tickers.find { |t| t.base_asset_id == @base0.id }
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
    create(:transaction, bot: @bot, base: 'BTC', quote: 'USD',
                         status: :submitted, external_status: :closed)
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
    other = create(:dca_dual_asset, status: :executing, exchange: @bot.exchange,
                                    base0_asset: create(:asset, symbol: 'SOL', name: 'Solana'),
                                    base1_asset: create(:asset, symbol: 'ADA', name: 'Cardano'),
                                    quote_asset: Asset.find(@bot.quote_asset_id))
    job = enqueue_job_for(other)
    run!

    assert_equal other.to_global_id.to_s, gid_in(job)
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

  test 'a bot with an order still live on a venue is left alone' do
    create(:transaction, bot: @bot, status: :submitted, external_status: :open)
    _, skipped = run!

    # The reason matters: without it this passes just as well when the bot was skipped for a stray
    # metrics job instead.
    assert_equal 'Bots::DcaDualAsset', Bot.where(id: @bot.id).pick(:type)
    assert_includes skipped.map(&:last), 'live order'
  end

  test 'a settled order does not block conversion' do
    create(:transaction, bot: @bot, status: :submitted, external_status: :closed)
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
    Ticker.find_by(exchange: @bot.exchange, base_asset_id: @base1.id).update!(trading_enabled: false)
    _, skipped = run!

    assert_equal 'Bots::DcaDualAsset', Bot.where(id: @bot.id).pick(:type)
    assert_includes skipped.map(&:last), 'missing ticker'
    assert_equal 0, BotIndexAsset.where(bot_id: @bot.id).count
  end

  test 'an unexpected composition row blocks conversion rather than adding a member' do
    stray = create(:asset, symbol: 'SOL', name: 'Solana')
    BotIndexAsset.create!(bot_id: @bot.id, asset_id: stray.id, ticker_id: Ticker.first.id,
                          in_index: true, target_allocation: 0.1)
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
