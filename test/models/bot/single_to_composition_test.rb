require 'test_helper'

# The in-place conversion of a single-asset bot into a one-asset multi-asset bot. What matters is what
# SURVIVES — the carry, the schedule anchor, the conditions, the sell sentence, the history, the queued
# jobs — and what the conversion refuses rather than guesses.
class Bot::SingleToCompositionTest < ActiveSupport::TestCase
  SINGLE = 'Bots::DcaSingleAsset'.freeze
  MULTI = 'Bots::DcaMultiAsset'.freeze

  setup do
    # Transaction's after_commit enqueues Bot::UpdateMetricsJob with the bot's GlobalID in Ready, which
    # busy_job? reads as a job in flight. Stubbed before any fixture writes a row.
    Bot::UpdateMetricsJob.stubs(:perform_later)
    @pair = create(:dca_single_asset, status: :stopped, started_at: 2.days.ago)
    @btc = @pair.base_asset
    @usd = @pair.quote_asset
    @ticker = @pair.ticker
  end

  def run! = Bot::SingleToComposition.run!
  def row = Bot::SingleToComposition::Row.find(@pair.id)
  def converted = Bot.find(@pair.id)
  def type_of(id) = Bot::SingleToComposition::Row.where(id:).pick(:type)
  def write_settings!(**pairs) = row.update_columns(settings: row.settings.merge(pairs.stringify_keys))
  def skip_reasons(result) = result.last.map(&:last)

  def order!(**columns)
    create(:transaction, bot: @pair, exchange: @pair.exchange, status: :submitted, external_status: :closed,
                         side: :buy, external_id: "o-#{SecureRandom.hex(4)}", price: 100, amount: 1,
                         amount_exec: 1, quote_amount: 100, quote_amount_exec: 100, **columns)
  end

  # A queued job addressed to a bot under a class name, in a given execution state — built by hand so
  # the GlobalID names the class under test, not the row's current one.
  def enqueue_job_for(id, klass: SINGLE, state: :none, scheduled_at: 1.hour.from_now)
    job = SolidQueue::Job.create!(
      queue_name: 'default', class_name: 'Bot::ActionJob', priority: 0,
      arguments: { 'job_class' => 'Bot::ActionJob', 'arguments' => [{ '_aj_globalid' => "gid://deltabadger/#{klass}/#{id}" }] }
    )
    SolidQueue::ReadyExecution.where(job_id: job.id).delete_all unless state == :ready
    case state
    when :claimed
      process = SolidQueue::Process.create!(kind: 'Worker', pid: 1, name: "worker-#{job.id}", last_heartbeat_at: Time.current)
      SolidQueue::ClaimedExecution.create!(job_id: job.id, process_id: process.id)
    when :blocked
      job.update!(concurrency_key: "bot_#{id}")
      SolidQueue::BlockedExecution.create!(job_id: job.id, queue_name: job.queue_name, priority: job.priority,
                                           concurrency_key: job.concurrency_key, expires_at: 5.minutes.from_now)
    when :scheduled
      job.update!(scheduled_at:)
      SolidQueue::ScheduledExecution.create!(job_id: job.id, queue_name: job.queue_name, priority: job.priority,
                                             scheduled_at:)
    end
    job
  end

  def gid_in(job) = job.reload.arguments['arguments'].first['_aj_globalid']

  # == What the conversion produces ==

  test 'a single-asset bot becomes a one-asset multi-asset bot on its own ticker' do
    converted_ids, skipped = run!

    assert_equal [[@pair.id], []], [converted_ids, skipped]
    bot = converted
    assert_equal MULTI, bot.type
    assert_equal({ @btc.id.to_s => 1.0 }, bot.allocations)
    assert_not bot.settings.key?('base_asset_id')
    membership = bot.bot_index_assets.sole
    assert_equal [@btc.id, @ticker.id, true, 1.0], membership.values_at(:asset_id, :ticker_id, :in_index, :target_allocation)
    assert_equal @pair.created_at.to_i, membership.entered_at.to_i
  end

  test 'a sell sentence left blank is written as the base amount the single-asset bot read it as' do
    run!

    assert_equal 'base', converted.settings['sell_denomination']
  end

  test 'a sell sentence already chosen is kept' do
    write_settings!(sell_denomination: 'quote')
    run!

    assert_equal 'quote', converted.sell_denomination
  end

  test 'a selling bot keeps selling its base amount' do
    write_settings!(direction: 'selling', sell_amount: '0.01')
    run!

    assert converted.sells_base_amount?
    assert_equal 0.01.to_d, converted.sell_amount
  end

  test 'the carry, the schedule anchor, the settings clock, transient data and status are kept' do
    anchor = 3.days.ago.change(usec: 0)
    clock = 1.day.ago.change(usec: 0)
    row.update_columns(started_at: anchor, settings_changed_at: clock, status: Bot.statuses[:scheduled],
                       transient_data: row.transient_data.merge('missed_quote_amount' => 42.0, 'marker' => 'x'))
    run!
    bot = converted

    assert_equal [anchor.to_i, clock.to_i, 'scheduled'], [bot.started_at.to_i, bot.settings_changed_at.to_i, bot.status]
    assert_equal [42.0, 'x'], [bot.missed_quote_amount.to_f, bot.transient_data['marker']]
  end

  test 'conditions on the bot’s own ticker survive' do
    write_settings!(price_limited: true, price_limit: 50_000.0, price_limit_in_ticker_id: @ticker.id)
    run!

    assert converted.price_limited?
    assert_equal [50_000.0, @ticker.id], [converted.price_limit, converted.price_limit_in_ticker_id]
  end

  test 'transactions are untouched' do
    order!(base: 'BTC', quote: 'USD')
    before = Transaction.where(bot_id: @pair.id).pluck(:id, :base, :base_asset_id, :quote_asset_id, :amount_exec)
    run!

    assert_equal before, Transaction.where(bot_id: @pair.id).pluck(:id, :base, :base_asset_id, :quote_asset_id, :amount_exec)
  end

  test 'rows recorded under an older symbol of the same asset convert, and read the same' do
    order!(base: 'OLDBTC', price: 100, resolve_asset_ids: false, base_asset_id: @btc.id, quote_asset_id: @usd.id)
    order!(base: 'BTC', price: 50)
    before = @pair.metrics(force: true).values_at(:total_quote_amount_invested, :total_base_amount)
    run!
    after = converted.metrics(force: true)

    assert_equal MULTI, converted.type
    assert_equal before, [after[:total_quote_amount_invested], after[:asset_breakdown]['BTC'][:amount]]
  end

  test 'a stopped bot on a delisted ticker converts, and its page still has its precision' do
    @ticker.update!(available: false)
    run!

    assert_equal MULTI, converted.type
    assert converted.decimals[:base]
  end

  # == What is refused ==

  test 'a working bot on a ticker the venue no longer trades waits' do
    row.update_columns(status: Bot.statuses[:scheduled])
    @ticker.update!(trading_enabled: false)

    assert_includes skip_reasons(run!), 'ticker not tradeable'
    assert_equal SINGLE, type_of(@pair.id)
  end

  test 'an executing bot is left alone' do
    row.update_columns(status: Bot.statuses[:executing])

    assert_includes skip_reasons(run!), 'executing'
    assert_equal SINGLE, type_of(@pair.id)
  end

  %i[claimed ready blocked].each do |state|
    test "a bot whose job is #{state} is left alone" do
      enqueue_job_for(@pair.id, state:)

      assert_includes skip_reasons(run!), 'job in flight'
      assert_equal SINGLE, type_of(@pair.id)
    end
  end

  test 'a scheduled job already due holds the bot back' do
    enqueue_job_for(@pair.id, state: :scheduled, scheduled_at: 1.minute.ago)

    assert_includes skip_reasons(run!), 'job in flight'
  end

  test 'a job scheduled for later does not, and is repointed so it still finds the bot' do
    job = enqueue_job_for(@pair.id, state: :scheduled)
    run!

    assert_equal MULTI, converted.type
    assert_equal "gid://deltabadger/#{MULTI}/#{@pair.id}", gid_in(job)
    assert_equal @pair.id, GlobalID::Locator.locate(gid_in(job)).id
  end

  test 'an enabled condition watching another ticker is refused, on either side' do
    other = create(:ticker, exchange: @pair.exchange, base_asset: create(:asset, :ethereum), quote_asset: @usd)
    { price_limited: :price_limit_in_ticker_id, sell_indicator_limited: :sell_indicator_limit_in_ticker_id }
      .each do |flag, key|
      write_settings!(flag => true, key => other.id)

      assert_includes skip_reasons(run!), 'condition watches another ticker'
      assert_equal SINGLE, type_of(@pair.id)
      write_settings!(flag => false)
    end
  end

  test 'a disabled condition watching another ticker is not a reason to wait' do
    other = create(:ticker, exchange: @pair.exchange, base_asset: create(:asset, :ethereum), quote_asset: @usd)
    write_settings!(price_drop_limited: false, price_drop_limit_in_ticker_id: other.id)
    run!

    assert_equal MULTI, converted.type
  end

  test 'a membership row of another asset is refused rather than joining the composition' do
    eth = create(:asset, :ethereum)
    eth_ticker = create(:ticker, exchange: @pair.exchange, base_asset: eth, quote_asset: @usd)
    BotIndexAsset.insert!({ bot_id: @pair.id, asset_id: eth.id, ticker_id: eth_ticker.id, in_index: true,
                            target_allocation: 0.1 })

    assert_includes skip_reasons(run!), 'unexpected composition rows'
  end

  test 'a row of another asset, or of none, is refused' do
    stray = order!(base: 'BTC')
    [create(:asset, :ethereum).id, nil].each do |asset_id|
      stray.update_columns(base_asset_id: asset_id)

      assert_includes skip_reasons(run!), 'orders of another asset'
      assert_equal SINGLE, type_of(@pair.id)
    end
  end

  test 'a rebalance, redeploy or liquidation row is refused' do
    order!(base: 'BTC', transaction_type: 'REBALANCE')

    assert_includes skip_reasons(run!), 'non-regular orders'
  end

  test 'a sell executed without reported proceeds is refused' do
    order!(base: 'BTC', side: :sell, quote_amount_exec: nil)

    assert_includes skip_reasons(run!), 'unpriced sell'
  end

  test 'a closed sell that reported neither its executed amount nor its proceeds is refused' do
    order!(base: 'BTC', side: :sell, amount_exec: nil, quote_amount_exec: nil)

    assert_includes skip_reasons(run!), 'unpriced sell'
  end

  test 'a bot whose pair has no ticker row at all is refused' do
    write_settings!(quote_asset_id: create(:asset, :eur).id)

    assert_includes skip_reasons(run!), 'missing ticker'
  end

  test 'a bot missing its base asset is refused' do
    write_settings!(base_asset_id: nil)

    assert_includes skip_reasons(run!), 'missing assets'
  end

  # == Recovery ==

  test 'running it twice changes nothing the second time' do
    run!
    snapshot = converted.attributes
    memberships = BotIndexAsset.where(bot_id: @pair.id).pluck(:id, :updated_at)
    run!

    assert_equal snapshot, converted.attributes
    assert_equal memberships, BotIndexAsset.where(bot_id: @pair.id).pluck(:id, :updated_at)
  end

  test 'a conversion interrupted before its queue write is finished by a re-run and by the recurring job' do
    job = enqueue_job_for(@pair.id)
    Bot::SingleToComposition::Row.where(id: @pair.id).update_all(type: MULTI)

    Bot::ConvertSingleAssetBotsJob.perform_now

    assert_equal "gid://deltabadger/#{MULTI}/#{@pair.id}", gid_in(job)
  end

  test 'a stale single-asset save after the flip is repaired, weights and sell sentence restored' do
    run!
    # What a request holding a pre-flip instance writes: its whole settings hash, pair-shaped.
    Bot::SingleToComposition::Row.where(id: @pair.id).update_all(
      settings: converted.settings.except('allocations', 'sell_denomination').merge('base_asset_id' => @btc.id)
    )

    run!
    bot = converted

    assert_equal({ @btc.id.to_s => 1.0 }, bot.allocations)
    assert_equal 'base', bot.settings['sell_denomination']
    assert_not bot.settings.key?('base_asset_id')
  end

  test 'a repair waits while a job naming the converted bot is claimed' do
    run!
    Bot::SingleToComposition::Row.where(id: @pair.id).update_all(
      settings: converted.settings.except('allocations').merge('base_asset_id' => @btc.id)
    )
    enqueue_job_for(@pair.id, klass: MULTI, state: :claimed)

    run!

    assert_not Bot::SingleToComposition::Row.find(@pair.id).settings.key?('allocations')
  end

  test 'a repair leaves alone a row a legitimate save repaired after it was selected' do
    run!
    clobbered = Bot::SingleToComposition::Row.find(@pair.id)
    clobbered.update_columns(settings: clobbered.settings.except('allocations').merge('base_asset_id' => @btc.id))
    selected = Bot::SingleToComposition.clobbered.to_a.sole
    Bot::SingleToComposition::Row.find(@pair.id).update_columns(
      settings: { 'allocations' => { @btc.id.to_s => 1.0 }, 'quote_asset_id' => @usd.id, 'quote_amount' => 20.0,
                  'interval' => 'day', 'sell_denomination' => 'quote' }
    )

    assert_kind_of String, Bot::SingleToComposition.convert!(selected, from_type: MULTI)
    assert_equal 'quote', converted.settings['sell_denomination'], 'the save that repaired it stands'
  end

  test 'finding a converted bot by its old class returns the basket' do
    run!

    assert_equal MULTI, Bots::DcaSingleAsset.find(@pair.id).type
  end

  test 'the recurring job converts a bot that was busy at the first pass' do
    job = enqueue_job_for(@pair.id, state: :ready)
    run!
    assert_equal SINGLE, type_of(@pair.id)

    SolidQueue::ReadyExecution.where(job_id: job.id).delete_all
    Bot::ConvertSingleAssetBotsJob.perform_now

    assert_equal MULTI, type_of(@pair.id)
  end

  test 'a row that raises is reported, and the next one still converts' do
    other = create(:dca_single_asset, status: :stopped, exchange: create(:kraken_exchange), base_asset: @btc,
                                      quote_asset: @usd)
    failing = @pair.id
    Bot::SingleToComposition.stubs(:busy_job?).returns(false)
    Bot::SingleToComposition.stubs(:busy_job?).with { |id, *| id == failing }.raises(RuntimeError, 'boom')

    converted_ids, skipped = run!

    assert_includes skipped, [@pair.id, 'error: boom']
    assert_includes converted_ids, other.id
  end

  test 'the migration converts, and runs on a database with no single-asset bots' do
    require Rails.root.glob('db/migrate/*_migrate_single_asset_bots_to_multi_asset.rb').sole

    capture_io { MigrateSingleAssetBotsToMultiAsset.new.up }
    assert_equal MULTI, type_of(@pair.id)
    assert_nothing_raised { capture_io { MigrateSingleAssetBotsToMultiAsset.new.up } }
  end
end
