# frozen_string_literal: true

require 'test_helper'

# Several DCA bots collapse into one basket that owns all of their history. What matters is what the
# new bot is made of (the anchor's schedule, the summed weights, every rule off), what moves with it
# (orders, activity, memberships, the two pieces of state the walk needs) and what the merge refuses.
class Bot::MergeTest < ActiveSupport::TestCase
  setup do
    # Transaction's after_commit enqueues Bot::UpdateMetricsJob with the bot's GlobalID in Ready, which
    # the quiescence check reads as a job in flight. Stubbed before any fixture writes a row.
    Bot::UpdateMetricsJob.stubs(:perform_later)
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @usd = create(:asset, :usd)
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum)
    @sol = create(:asset, symbol: 'SOL', name: 'Solana', external_id: 'solana')
  end

  # == What the merge produces ==

  test 'two baskets become one stopped basket on the anchor schedule, named like the wizard names it' do
    anchor = basket([@btc])
    write_settings!(anchor, interval: 'week', quote_amount: 250)
    other = basket([@eth, @sol])
    other.update_columns(position: anchor.position + 1)

    merged = merge!(anchor, other)

    assert_instance_of Bots::DcaMultiAsset, merged
    assert_predicate merged, :stopped?
    assert_equal @exchange, merged.exchange
    assert_equal @usd.id, merged.quote_asset_id.to_i
    assert_equal 'week', merged.interval
    assert_equal 250, merged.quote_amount.to_d
    assert_equal anchor.position, merged.position
    assert_equal 'BTC, ETH, SOL', merged.label
    assert_equal 'manual', merged.weighting
    assert_equal @user, merged.user
  end

  test 'weights are the sum of the sources current target weights, normalised' do
    anchor = basket([@btc])
    other = basket([@eth, @sol], allocations: { @eth => 0.6, @sol => 0.4 })

    merged = merge!(anchor, other)

    assert_equal({ @btc.id.to_s => 0.5, @eth.id.to_s => 0.3, @sol.id.to_s => 0.2 }, merged.allocations)
    assert_in_delta 1, merged.allocations.values.sum, 0.0001
    targets = merged.bot_index_assets.in_index.pluck(:asset_id, :target_allocation).to_h
    assert_in_delta 0.5, targets[@btc.id], 0.0001
    assert_in_delta 0.3, targets[@eth.id], 0.0001
    assert_in_delta 0.2, targets[@sol.id], 0.0001
  end

  test 'an asset held by both sources appears once with the summed weight' do
    anchor = basket([@btc, @eth], allocations: { @btc => 0.7, @eth => 0.3 })
    other = basket([@eth])

    merged = merge!(anchor, other)

    assert_equal({ @btc.id.to_s => 0.35, @eth.id.to_s => 0.65 }, merged.allocations)
    assert_equal 1, merged.bot_index_assets.where(asset_id: @eth.id).count
  end

  test 'an index bot contributes its current target allocations and leaves its index settings behind' do
    anchor = basket([@btc])
    index = index_bot
    membership!(index, @eth, 0.75)
    membership!(index, @sol, 0.25)

    merged = merge!(anchor, index)

    assert_equal({ @btc.id.to_s => 0.5, @eth.id.to_s => 0.375, @sol.id.to_s => 0.125 }, merged.allocations)
    %w[num_coins index_type allocation_flattening index_category_id index_name hold_all].each do |key|
      assert_not merged.settings.key?(key), "#{key} leaked into the basket"
    end
  end

  test 'every rule is off on the merged bot, whatever the anchor had switched on' do
    anchor = basket([@btc, @eth])
    ticker = Ticker.find_by!(exchange: @exchange, base_asset: @btc, quote_asset: @usd)
    write_settings!(anchor, smart_intervaled: true, smart_interval_quote_amount: 10,
                            limit_ordered: true, limit_order_pcnt_distance: 0.01,
                            quote_amount_limited: true, quote_amount_limit: 500,
                            price_limited: true, price_limit: 1, price_limit_in_ticker_id: ticker.id,
                            sell_price_limited: true, sell_price_limit: 2, sell_price_limit_in_ticker_id: ticker.id,
                            price_drop_limited: true, moving_average_limited: true, indicator_limited: true,
                            rebalance_enabled: true, rebalance_threshold: 0.1,
                            weighting: 'market_cap', direction: 'selling', sell_quote_amount: 20,
                            start_time_enabled: true, start_time_mode: 'time_of_day', start_time_of_day: '10:00')
    other = basket([@sol])

    merged = merge!(anchor, other)

    assert_not merged.smart_intervaled?
    assert_not merged.limit_ordered
    assert_not merged.quote_amount_limited
    assert_not merged.price_limited?
    assert_not merged.sell_price_limited?
    assert_not merged.settings['price_drop_limited']
    assert_not merged.settings['moving_average_limited']
    assert_not merged.settings['indicator_limited']
    assert_not merged.rebalance_enabled?
    assert_not merged.start_time_enabled?
    assert_equal 'manual', merged.weighting
    assert_equal 'buying', merged.direction
    assert_not merged.selling?
    assert_nil merged.settings['sell_quote_amount']
  end

  # == What moves ==

  test 'every order and activity row of every source belongs to the merged bot, and the feed says so' do
    anchor = basket([@btc])
    other = basket([@eth])
    rows = [
      order!(anchor, base: 'BTC'),
      order!(anchor, base: 'BTC', transaction_type: 'REBALANCE', side: :sell),
      order!(anchor, base: 'BTC', transaction_type: 'REBALANCE'), # the swap's buy leg: no cash left in flight
      order!(other, base: 'ETH'),
      order!(other, base: 'ETH', transaction_type: 'LIQUIDATION', side: :sell),
      order!(other, base: 'ETH', transaction_type: 'REDEPLOY'),
      create(:transaction, :open, bot: other, exchange: @exchange, base: 'ETH', quote: 'USD')
    ]
    anchor.log_activity('started')
    other.log_activity('stopped')

    merged = merge!(anchor, other)

    assert_equal [merged.id], rows.map { |row| row.reload.bot_id }.uniq
    assert_equal 0, Transaction.where(bot_id: [anchor.id, other.id]).count
    assert_equal 0, BotActivityLog.where(bot_id: [anchor.id, other.id]).count
    assert_equal %w[started stopped], merged.bot_activity_logs.where(event: %w[started stopped]).order(:id).pluck(:event)
    merged_log = merged.bot_activity_logs.find_by(event: 'merged')
    assert_equal [anchor.id, other.id], merged_log.details['source_ids']
    assert_equal [anchor.label, other.label], merged_log.details['source_labels']
  end

  test 'memberships fold: one row per asset, the earliest entry, exited rows carried as exited' do
    anchor = basket([@btc, @eth])
    other = basket([@eth])
    anchor.bot_index_assets.find_by(asset_id: @eth.id).update_columns(entered_at: 3.weeks.ago)
    other.bot_index_assets.find_by(asset_id: @eth.id).update_columns(entered_at: 1.week.ago)
    # SOL left the anchor's basket a while ago; its holding is still priced and sellable through this row.
    sol_ticker = ticker_for(@sol)
    anchor.bot_index_assets.create!(asset: @sol, ticker: sol_ticker, in_index: false,
                                    entered_at: 2.months.ago, exited_at: 1.month.ago, target_allocation: 0.2)

    merged = merge!(anchor, other)

    eth = merged.bot_index_assets.where(asset_id: @eth.id).sole
    assert_predicate eth, :in_index?
    assert_in_delta 3.weeks.ago, eth.entered_at, 5.seconds
    sol = merged.bot_index_assets.where(asset_id: @sol.id).sole
    assert_not sol.in_index?
    assert_equal sol_ticker, sol.ticker
    assert_in_delta 1.month.ago, sol.exited_at, 5.seconds
    assert_in_delta 2.months.ago, sol.entered_at, 5.seconds
    assert_not_includes merged.base_asset_ids, @sol.id
  end

  test 'an asset exited on one source but held by another is a member' do
    anchor = basket([@btc])
    anchor.bot_index_assets.create!(asset: @eth, ticker: ticker_for(@eth), in_index: false,
                                    entered_at: 2.months.ago, exited_at: 1.month.ago)
    other = basket([@eth])

    merged = merge!(anchor, other)

    assert_includes merged.base_asset_ids, @eth.id
    assert_predicate merged.bot_index_assets.where(asset_id: @eth.id).sole, :in_index?
  end

  test 'a member that no longer trades at the quote is dropped from the weights and kept as an exited holding' do
    anchor = basket([@btc, @eth])
    other = basket([@sol])
    Ticker.find_by!(exchange: @exchange, base_asset: @eth, quote_asset: @usd).update_columns(trading_enabled: false)

    merged = merge!(anchor, other)

    # BTC 0.5 + SOL 1.0 with ETH's 0.5 gone, normalised.
    assert_equal({ @btc.id.to_s => 0.333, @sol.id.to_s => 0.667 }, merged.allocations)
    eth = merged.bot_index_assets.where(asset_id: @eth.id).sole
    assert_not eth.in_index?
    assert_not_nil eth.exited_at
  end

  test 'when nothing trades any more the merge refuses and writes nothing' do
    anchor = basket([@btc])
    other = basket([@eth])
    Ticker.where(exchange: @exchange, quote_asset: @usd).update_all(available: false)

    merge = Bot::Merge.new(@user, [anchor.id, other.id])

    assert_equal :nothing_to_buy, merge.reason
    assert_no_difference 'Bot.count' do
      assert_nil merge.perform!
    end
    assert_predicate anchor.reload, :stopped?
  end

  test 'sources are soft-deleted, their scheduled ticks cancelled, and the merged bot has nothing scheduled' do
    anchor = basket([@btc], status: :scheduled, started_at: 1.day.ago)
    other = basket([@eth])
    job = enqueue_job_for(anchor, state: :scheduled, scheduled_at: 2.hours.from_now)

    merged = merge!(anchor, other)

    assert_predicate anchor.reload, :deleted?
    assert_predicate other.reload, :deleted?
    assert_not_nil anchor.stopped_at
    assert_not SolidQueue::ScheduledExecution.exists?(job_id: job.id)
    assert_nil merged.next_action_job_at
  end

  test 'declined and reinvested proceeds stay declined on the merged bot, and attested liquidation orders stay attested' do
    anchor = basket([@btc])
    other = basket([@eth])
    order!(anchor, base: 'BTC')
    order!(anchor, base: 'BTC', side: :sell, transaction_type: 'LIQUIDATION', quote_amount: 40, quote_amount_exec: 40)
    assert anchor.decline_redeploy!.success?
    order!(anchor, base: 'BTC', quote_amount: 40, quote_amount_exec: 40) # the next contribution used the cash up
    order!(other, base: 'ETH')
    order!(other, base: 'ETH', side: :sell, transaction_type: 'LIQUIDATION', quote_amount: 120, quote_amount_exec: 120)
    assert other.decline_redeploy!.success?
    order!(other, base: 'ETH', quote_amount: 120, quote_amount_exec: 120)
    abandoned = create(:transaction, bot: other, exchange: @exchange, base: 'ETH', quote: 'USD', side: :sell,
                                     transaction_type: 'LIQUIDATION', status: :submitted, external_status: :abandoned,
                                     amount_exec: nil, quote_amount_exec: nil)
    other.merge_transient_data!(Bot::LiquidationState::RESOLVED_KEY => [abandoned.id])

    merged = merge!(anchor, other)

    assert_equal 160, merged.redeploy_declined_offset.to_d
    assert_equal 0, merged.redeploy_offer, 'nothing the sources had declined is offered again'
    assert_equal [abandoned.id], merged.resolved_liquidation_order_ids
    assert_not merged.liquidation_halted?
  end

  test 'a source counting down to its next tick is mergeable: the tick is cancelled first, then the history moves' do
    anchor = basket([@btc])
    other = basket([@eth], status: :scheduled, started_at: 1.day.ago)
    later = enqueue_job_for(other, state: :scheduled, scheduled_at: 2.hours.from_now)
    # As if the dispatcher had parked the tick behind the venue lock the merge holds.
    parked = enqueue_job_for(other, state: :blocked)
    order!(other, base: 'ETH')

    merged = merge!(anchor, other)

    assert_equal 1, merged.transactions.count
    assert_predicate other.reload, :deleted?
    assert_not SolidQueue::Job.exists?(id: [later.id, parked.id])
    assert_equal 0, SolidQueue::Job.where('arguments LIKE ?', "%#{other.to_global_id}\"%").where(finished_at: nil).count
  end

  test 'the merge holds the venue trading lock while it writes, and hands it back' do
    anchor = basket([@btc])
    other = basket([@eth])
    key = Bot::Merge::ExchangeLease.for(@exchange).concurrency_key
    held = nil
    probe = Class.new(Bot::Merge) do
      define_method(:write!) do
        held = SolidQueue::Semaphore.find_by(key:)&.value
        super()
      end
    end

    merged = probe.new(@user, [anchor.id, other.id]).perform!

    assert merged
    assert_equal 0, held, 'the semaphore every trading job on the venue waits for was ours during the write'
    assert_equal 1, SolidQueue::Semaphore.find_by(key:).value, 'and it is free again'
  end

  test 'a write that fails leaves the sources with their ticks, and hands the venue back' do
    anchor = basket([@btc], status: :scheduled, started_at: 1.day.ago)
    other = basket([@eth])
    later = enqueue_job_for(anchor, state: :scheduled, scheduled_at: 2.hours.from_now)
    key = Bot::Merge::ExchangeLease.for(@exchange).concurrency_key
    broken = Class.new(Bot::Merge) do
      define_method(:write!) { raise ActiveRecord::RecordInvalid }
    end

    assert_raises(ActiveRecord::RecordInvalid) { broken.new(@user, [anchor.id, other.id]).perform! }

    assert_predicate anchor.reload, :scheduled?
    assert_predicate other.reload, :stopped?
    assert SolidQueue::ScheduledExecution.exists?(job_id: later.id), 'the tick the merge would have cancelled is still there'
    assert_equal 1, SolidQueue::Semaphore.find_by(key:).value
  end

  test 'a merge that outlives its lease does not signal a semaphore that may be someone elses' do
    anchor = basket([@btc])
    other = basket([@eth])
    key = Bot::Merge::ExchangeLease.for(@exchange).concurrency_key
    # The probe owns its clock stubs: travel in write!, travel back once release has read the clock.
    # (A travel_back from the test would unstub nothing — TimeHelpers keeps stubs per object — and
    # leave the whole worker process five minutes in the future.)
    slow = Class.new(Bot::Merge) do
      include ActiveSupport::Testing::TimeHelpers

      define_method(:write!) do
        travel(Bot::Merge::LEASE + 1.second)
        super()
      end

      define_method(:release) do |*args|
        super(*args)
      ensure
        travel_back
      end
    end

    merged = slow.new(@user, [anchor.id, other.id]).perform!

    assert merged
    assert_equal 0, SolidQueue::Semaphore.find_by(key:).value, 'left for the queue to expire, never signalled'
  end

  test 'a limit check that finishes after its source was merged away does not re-arm it' do
    anchor = basket([@btc])
    other = basket([@eth], status: :waiting, started_at: 1.day.ago)
    ticker = Ticker.find_by!(exchange: @exchange, base_asset: @eth, quote_asset: @usd)
    write_settings!(other, price_limited: true, price_limit: 1, price_limit_in_ticker_id: ticker.id)
    stale = Bot.find(other.id) # what a running check holds while it reads the price
    Bot::PriceLimitCheckJob.any_instance.stubs(:condition_result).returns(Result::Success.new(true))

    merged = merge!(anchor, other)
    Bot::PriceLimitCheckJob.new.perform(stale)

    assert_predicate other.reload, :deleted?
    assert_predicate merged.reload, :stopped?
    assert_equal 0, SolidQueue::Job.where(class_name: 'Bot::ActionJob').where('arguments LIKE ?', "%#{other.to_global_id}\"%").count
  end

  test 'a bot still waiting when its check completes is re-armed, once' do
    bot = basket([@btc], status: :waiting, started_at: 1.day.ago)
    ticker = Ticker.find_by!(exchange: @exchange, base_asset: @btc, quote_asset: @usd)
    write_settings!(bot, price_limited: true, price_limit: 1, price_limit_in_ticker_id: ticker.id)
    Bot::PriceLimitCheckJob.any_instance.stubs(:condition_result).returns(Result::Success.new(true))

    Bot::PriceLimitCheckJob.new.perform(bot)

    assert_predicate bot.reload, :scheduled?
    assert_equal 1, SolidQueue::Job.where(class_name: 'Bot::ActionJob').where('arguments LIKE ?', "%#{bot.to_global_id}\"%").count
  end

  test 'the merged bot tells its own first tick apart from the history it inherited, whatever the clock says' do
    anchor = basket([@btc])
    other = basket([@eth])
    order!(anchor, base: 'BTC')
    last = order!(other, base: 'ETH')

    merged = merge!(anchor, other)

    assert_equal 2, merged.transactions.count
    assert_equal last.id, merged.transient_data[Bot::Composition::OrderSetter::MERGED_HISTORY_KEY]
    assert_predicate merged.own_transactions, :none?
    # Placed in the very second the bot was created: still its own.
    own = create(:transaction, :skipped, bot: merged, exchange: @exchange, base: 'BTC', quote: 'USD', created_at: merged.created_at)
    assert_equal [own.id], merged.own_transactions.pluck(:id)
    # A bot that inherited nothing reads every row of its own.
    plain = basket([@sol])
    row = order!(plain, base: 'SOL')
    assert_equal [row.id], plain.own_transactions.pluck(:id)
  end

  test 'a Start that loaded the source before the merge cannot bring it back' do
    anchor = basket([@btc])
    other = basket([@eth])
    stale = Bot.find(other.id)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    merged = merge!(anchor, other)
    Bot::ActionJob.expects(:perform_later).never
    started = stale.start(start_fresh: true)

    assert_not started
    assert_predicate other.reload, :deleted?
    assert_predicate merged.reload, :stopped?
  end

  # == The merged history is the merged bot's books ==

  test 'a sale of an asset both sources hold is realised at the pooled cost' do
    anchor = basket([@btc])
    other = basket([@btc])
    t = 2.days.ago
    order!(anchor, base: 'BTC', created_at: t)
    order!(other, base: 'BTC', created_at: t + 1.hour, price: 50, quote_amount: 50, quote_amount_exec: 50)
    order!(other, base: 'BTC', side: :sell, created_at: t + 2.hours, price: 80, quote_amount: 80, quote_amount_exec: 80)
    assert_equal 30, other.metrics(force: true)[:realised_pnl].to_d, 'on its own: against its own cost of 50'

    books = merge!(anchor, other).metrics(force: true)

    assert_equal 5, books[:realised_pnl].to_d, 'against the pooled cost of 75'
    assert_equal 150, books[:total_quote_amount_invested].to_d
    assert_equal 1, books[:asset_breakdown]['BTC'][:amount].to_d
  end

  # Proceeds wait for the user on one bot; pooling two histories must not answer for them.
  test 'a buy after another source kept its sale proceeds leaves them on offer' do
    anchor = basket([@btc])
    other = basket([@eth])
    t = 2.days.ago
    order!(other, base: 'ETH', created_at: t)
    order!(other, base: 'ETH', side: :sell, transaction_type: 'LIQUIDATION', created_at: t + 1.hour, price: 120,
                  quote_amount: 120, quote_amount_exec: 120)
    order!(anchor, base: 'BTC', created_at: t + 2.hours)

    merged = merge!(anchor, other)

    books = merged.metrics(force: true)
    assert_equal 200, books[:total_quote_amount_invested].to_d, 'two contributions, both of them new money'
    assert_equal 120, books[:realised_cash].to_d
    assert_equal 120, merged.redeploy_offer(books).to_d, 'what the source was offering, still offered'
  end

  test 'a sale the venue never priced is estimated at the pooled cost' do
    anchor = basket([@btc])
    other = basket([@btc])
    t = 2.days.ago
    order!(anchor, base: 'BTC', created_at: t)
    order!(other, base: 'BTC', created_at: t + 1.hour, price: 50, quote_amount: 50, quote_amount_exec: 50)
    order!(other, base: 'BTC', side: :sell, transaction_type: 'LIQUIDATION', created_at: t + 2.hours,
                  quote_amount_exec: nil)
    assert_equal 50, other.metrics(force: true)[:rebalance_cash].to_d, 'on its own: estimated at its own cost of 50'

    books = merge!(anchor, other).metrics(force: true)

    assert_equal 75, books[:rebalance_cash].to_d, 'estimated at the pooled cost of 75'
    assert_equal 150, books[:total_quote_amount_invested].to_d
  end

  test 'a source that ever sold units it never bought cannot be merged' do
    anchor = basket([@btc])
    other = basket([@eth])
    order!(other, base: 'ETH', side: :sell, amount: 1, amount_exec: 1, quote_amount: 100, quote_amount_exec: 100)
    order!(other, base: 'ETH', quote_amount: 100, quote_amount_exec: 100) # the cash it left is spent again

    merge = Bot::Merge.new(@user, [anchor.id, other.id])

    assert_equal [:external_sales, { label: other.label }], merge.reason
    assert_nil merge.perform!
    assert_equal I18n.t('errors.bots.merge.external_sales', label: other.label), merge.error
  end

  test 'the venue-local cap reads a closed legacy fill at its ordered amount, as the ledger does' do
    kraken = create(:kraken_exchange)
    anchor = basket([@btc])
    on_kraken = create(:dca_multi_asset, user: @user, exchange: kraken, quote_asset: @usd, base_assets: [@eth],
                                         status: :stopped, with_api_key: false)
    ticker_for(@eth)
    order!(on_kraken, base: 'ETH', exchange: kraken, amount: 2, amount_exec: 2)
    merged = merge!(anchor, on_kraken)
    create(:transaction, bot: merged, exchange: @exchange, base: 'ETH', quote: 'USD', status: :submitted,
                         external_status: :closed, side: :buy, external_id: 'legacy', amount: 0.7,
                         amount_exec: nil, quote_amount: 70, quote_amount_exec: nil, resolve_asset_ids: true)
    merged.stubs(:get_balance).returns(Result::Success.new({ free: 5 }))

    assert_equal 0.7, merged.send(:live_free_balance, @eth.id)
  end

  test 'a combined history that loses money is refused after being measured, and nothing stays' do
    anchor = basket([@btc])
    other = basket([@eth])
    t = 1.day.ago
    # A buys, swaps out (cash in flight), B buys new money meanwhile, A's swap buys back: read as one
    # story, B's buy spends A's flight cash and A's buy-back spends money the books no longer have.
    order!(anchor, base: 'BTC', created_at: t)
    order!(anchor, base: 'BTC', side: :sell, transaction_type: 'REBALANCE', created_at: t + 1.hour)
    order!(other, base: 'ETH', created_at: t + 2.hours)
    order!(anchor, base: 'BTC', transaction_type: 'REBALANCE', created_at: t + 3.hours)
    merge = Bot::Merge.new(@user, [anchor.id, other.id])
    assert_nil merge.reason, 'nothing about either source alone gives it away'

    assert_no_difference 'Bot.count' do
      assert_nil merge.perform!
    end

    assert_equal I18n.t('errors.bots.merge.interleaved'), merge.error
    assert_predicate anchor.reload, :stopped?
    assert_equal 3, anchor.transactions.count, 'the rows went back with the rollback'
    assert_equal 1, SolidQueue::Semaphore.find_by(key: Bot::Merge::ExchangeLease.for(@exchange).concurrency_key).value
  end

  test 'a refused interleaving leaves no books cached under the id the rollback frees' do
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    anchor = basket([@btc])
    other = basket([@eth])
    t = 1.day.ago
    order!(anchor, base: 'BTC', created_at: t)
    order!(anchor, base: 'BTC', side: :sell, transaction_type: 'REBALANCE', created_at: t + 1.hour)
    order!(other, base: 'ETH', created_at: t + 2.hours)
    order!(anchor, base: 'BTC', transaction_type: 'REBALANCE', created_at: t + 3.hours)
    merge = Bot::Merge.new(@user, [anchor.id, other.id])
    freed_id = Bot.maximum(:id) + 1 # the id the merged bot takes, and the rollback gives back

    assert_nil merge.perform!

    assert_nil Rails.cache.read("bot_#{freed_id}_metrics_v9_0")
    # The next bot to take that id starts with books of its own.
    fresh = basket([@sol])
    assert_equal freed_id, fresh.id, 'SQLite hands the rolled-back id straight back'
    assert_empty fresh.metrics[:asset_breakdown]
  end

  test 'two different assets sharing a symbol merge, each kept apart' do
    # One venue cannot list two "ABC" at the same quote, so the second lives on Kraken — and has been
    # delisted there, which drops it from the weights and leaves its history as a holding.
    kraken = create(:kraken_exchange)
    abc_one = create(:asset, symbol: 'ABC', name: 'ABC One', external_id: 'abc-one')
    abc_two = create(:asset, symbol: 'ABC', name: 'ABC Two', external_id: 'abc-two')
    anchor = basket([@btc, abc_one])
    other = create(:dca_multi_asset, user: @user, exchange: kraken, quote_asset: @usd, base_assets: [abc_two],
                                     status: :stopped, with_api_key: false)
    Ticker.where(exchange: kraken, base_asset: abc_two).update_all(available: false)
    order!(anchor, base: 'ABC', amount: 1, amount_exec: 1, base_asset_id: abc_one.id, resolve_asset_ids: false)
    order!(other, base: 'ABC', exchange: kraken, amount: 2, amount_exec: 2, base_asset_id: abc_two.id,
                  resolve_asset_ids: false)

    merged = merge!(anchor, other)

    assert_equal [@btc.id, abc_one.id].sort, merged.base_asset_ids.sort
    units = merged.metrics(force: true)[:asset_breakdown].transform_values { |entry| entry[:amount].to_d }
    assert_equal [1, 2], units.values_at(*units.keys.grep(/ABC/)).sort, 'both holdings, told apart'
  end

  test 'a holding recorded without its asset merges beside the asset of the same name' do
    abc = create(:asset, symbol: 'ABC', name: 'ABC Coin', external_id: 'abc-coin')
    anchor = basket([@btc, abc])
    other = basket([@eth])
    order!(anchor, base: 'ABC', base_asset_id: abc.id, resolve_asset_ids: false)
    order!(other, base: 'ABC', base_asset_id: nil, resolve_asset_ids: false) # from before orders stored their asset

    merged = merge!(anchor, other)

    keys = merged.metrics(force: true)[:asset_breakdown].keys.grep(/ABC/)
    assert_equal ['ABC#?', "ABC##{abc.id}"].sort, keys.sort, 'on its own the legacy holding was plain ABC'
  end

  test 'money lost is measured in the quote, not in cents: a satoshi of it still refuses' do
    btc_quoted = create(:asset, symbol: 'WBTC', name: 'Wrapped BTC', external_id: 'wrapped-btc')
    anchor = create(:dca_multi_asset, user: @user, exchange: @exchange, quote_asset: @btc, base_assets: [@eth],
                                      status: :stopped)
    other = create(:dca_multi_asset, user: @user, exchange: @exchange, quote_asset: @btc, base_assets: [btc_quoted],
                                     status: :stopped)
    t = 1.day.ago
    fill = lambda do |bot, base, side: :buy, type: 'REGULAR', at: t|
      create(:transaction, bot:, exchange: @exchange, base:, quote: 'BTC', status: :submitted, external_status: :closed,
                           side:, transaction_type: type, external_id: "o-#{SecureRandom.hex(4)}", created_at: at,
                           price: 1, amount: 0.0000001, amount_exec: 0.0000001,
                           quote_amount: 0.0000001, quote_amount_exec: 0.0000001)
    end
    # The flight-cash story of the test above, at a satoshi's scale.
    fill.call(anchor, 'ETH')
    fill.call(anchor, 'ETH', side: :sell, type: 'REBALANCE', at: t + 1.hour)
    fill.call(other, 'WBTC', at: t + 2.hours)
    fill.call(anchor, 'ETH', type: 'REBALANCE', at: t + 3.hours)
    merge = Bot::Merge.new(@user, [anchor.id, other.id])

    assert_nil merge.perform!
    assert_equal I18n.t('errors.bots.merge.interleaved'), merge.error
  end

  test 'the merged books are the sum of the sources books when the histories do not interleave' do
    anchor = basket([@btc])
    other = basket([@eth])
    t = 2.days.ago
    order!(anchor, base: 'BTC', created_at: t, quote_amount: 100, quote_amount_exec: 100)
    order!(anchor, base: 'BTC', side: :sell, transaction_type: 'REBALANCE', created_at: t + 1.hour,
                   quote_amount: 110, quote_amount_exec: 110)
    order!(anchor, base: 'BTC', transaction_type: 'REBALANCE', created_at: t + 2.hours,
                   quote_amount: 110, quote_amount_exec: 110)
    order!(other, base: 'ETH', created_at: t + 1.day, quote_amount: 100, quote_amount_exec: 100)
    expected = anchor.metrics(force: true)[:total_quote_amount_invested].to_d +
               other.metrics(force: true)[:total_quote_amount_invested].to_d

    merged = merge!(anchor, other)

    assert_equal expected, merged.metrics(force: true)[:total_quote_amount_invested].to_d
  end

  test 'a share with rows from another venue is not sold from here at all: a split could have restated it' do
    kraken = create(:kraken_exchange)
    share = create(:asset, symbol: 'ACME', name: 'Acme Inc', external_id: 'acme', category: 'Stock')
    anchor = basket([@btc])
    on_kraken = create(:dca_multi_asset, user: @user, exchange: kraken, quote_asset: @usd, base_assets: [share],
                                         status: :stopped, with_api_key: false)
    ticker_for(share)
    order!(on_kraken, base: 'ACME', exchange: kraken, amount: 10, amount_exec: 10)
    merged = merge!(anchor, on_kraken)
    order!(merged, base: 'ACME', amount: 10, amount_exec: 10)
    merged.stubs(:get_balance).returns(Result::Success.new({ free: 20 }))

    assert_equal 0, merged.send(:live_free_balance, share.id)
  end

  test 'the venue-local cap only applies to an asset that has rows from another venue' do
    kraken = create(:kraken_exchange)
    anchor = basket([@btc])
    on_kraken = create(:dca_multi_asset, user: @user, exchange: kraken, quote_asset: @usd, base_assets: [@eth],
                                         status: :stopped, with_api_key: false)
    ticker_for(@eth)
    order!(anchor, base: 'BTC', amount: 1, amount_exec: 1)
    order!(on_kraken, base: 'ETH', exchange: kraken, amount: 2, amount_exec: 2)
    merged = merge!(anchor, on_kraken)
    merged.stubs(:get_balance).returns(Result::Success.new({ free: 5 }))

    assert_equal 0, merged.send(:live_free_balance, @eth.id), 'every ETH row is foreign'
    assert_equal 5, merged.send(:live_free_balance, @btc.id), 'every BTC row is local: the account as before'
  end

  test 'a poll that spans the merge refreshes the bot that owns the order now' do
    anchor = basket([@btc])
    other = basket([@eth])
    open = create(:transaction, :open, bot: other, exchange: @exchange, base: 'ETH', quote: 'USD',
                                       external_id: 'spanning', quote_amount: 100)
    ticker = Ticker.find_by!(exchange: @exchange, base_asset: @eth, quote_asset: @usd)
    fill = { status: :closed, price: 100, amount: 1, quote_amount: 100, amount_exec: 1, quote_amount_exec: 100,
             ticker:, side: :buy, order_type: :market_order }
    merged = nil
    # The venue answers only after the merge has moved the row.
    Bots::DcaMultiAsset.any_instance.stubs(:get_order)
                       .with { merged ||= merge!(anchor, Bot.find(other.id)) }
                       .returns(Result::Success.new(fill))
    refreshed = []
    Bot::UpdateMetricsJob.stubs(:perform_later).with { |bot| refreshed.push(bot.id) }

    Bot::FetchAndUpdateOrderJob.new.perform(open)

    assert_predicate open.reload, :closed?
    assert_equal merged.id, open.bot_id
    assert_includes refreshed, merged.id, 'the merged bot, whose page would otherwise keep the unfilled state'
    assert_not_includes refreshed, other.id
  end

  test 'a tick, rebalance, sale or redeploy running on the venue refuses the merge, and nothing is written' do
    anchor = basket([@btc])
    other = basket([@eth])
    # What Solid Queue leaves in place from dispatch to finish of any Bot::ActionJob-group job on Binance.
    SolidQueue::Semaphore.create!(key: Bot::Merge::ExchangeLease.for(@exchange).concurrency_key, value: 0,
                                  expires_at: 5.minutes.from_now)

    merge = Bot::Merge.new(@user, [anchor.id, other.id])

    assert_nil merge.reason, 'the tile cannot see the queue; only perform! measures it'
    assert_no_difference 'Bot.count' do
      assert_nil merge.perform!
    end
    assert_equal I18n.t('errors.bots.merge.unavailable', label: anchor.label), merge.error
    assert_predicate other.reload, :stopped?
  end

  test 'on Hyperliquid the merged bot keeps limit orders on, as that venue requires' do
    hyperliquid = create(:hyperliquid_exchange)
    usdc = create(:asset, symbol: 'USDC', name: 'USD Coin', external_id: 'usd-coin')
    # with_api_key: false — the factory's key is not a wallet, and the key is not what is under test.
    anchor = create(:dca_multi_asset, user: @user, exchange: hyperliquid, quote_asset: usdc, base_assets: [@btc],
                                      status: :stopped, with_api_key: false)
    other = create(:dca_multi_asset, user: @user, exchange: hyperliquid, quote_asset: usdc, base_assets: [@eth],
                                     status: :stopped, with_api_key: false)

    merged = merge!(anchor, other)

    assert_predicate merged, :limit_ordered?
    assert merged.valid?, merged.errors.full_messages.to_sentence
  end

  test 'perform! reads the anchor as it is now, not as the preview saw it' do
    anchor = basket([@btc])
    other = basket([@eth])
    merge = Bot::Merge.new(@user, [anchor.id, other.id])
    assert_nil merge.reason
    assert_equal 100, merge.bot.quote_amount.to_d
    write_settings!(anchor, quote_amount: 999)

    merged = merge.perform!

    assert merged, merge.error
    assert_equal 999, merged.quote_amount.to_d
  end

  # == What the merge refuses ==

  test 'fewer than two bots, a stranger id, and a bot deleted since the preview are refused as they are' do
    mine = basket([@btc])
    other = basket([@eth])
    theirs = create(:dca_multi_asset, user: create(:user), exchange: @exchange, quote_asset: @usd, base_assets: [@sol],
                                      status: :stopped)

    assert_equal :too_few, Bot::Merge.new(@user, [mine.id]).reason
    assert_equal :too_few, Bot::Merge.new(@user, [mine.id, mine.id]).reason
    assert_equal :missing, Bot::Merge.new(@user, [mine.id, theirs.id]).reason
    assert_equal :missing, Bot::Merge.new(@user, [mine.id, other.id, theirs.id]).reason
    assert_nil Bot::Merge.new(@user, [mine.id, theirs.id]).perform!
    assert_predicate theirs.reload, :stopped?

    merge = Bot::Merge.new(@user, [mine.id, other.id])
    assert_nil merge.reason
    other.update_columns(status: Bot.statuses[:deleted])
    assert_nil merge.perform!
    assert_equal I18n.t('errors.bots.merge.missing'), merge.error
    assert_predicate mine.reload, :stopped?
  end

  test 'another quote cannot join; another exchange can, when the anchor venue lists everything' do
    anchor = basket([@btc])
    eur = create(:asset, :eur)
    other_quote = create(:dca_multi_asset, user: @user, exchange: @exchange, quote_asset: eur, base_assets: [@sol],
                                           status: :stopped)
    assert_equal :quote, Bot::Merge.new(@user, [anchor.id, other_quote.id]).reason

    kraken = create(:kraken_exchange)
    elsewhere = create(:dca_multi_asset, user: @user, exchange: kraken, quote_asset: @usd, base_assets: [@eth],
                                         status: :stopped, with_api_key: false)
    ticker_for(@eth) # Binance lists ETH/USD too, so the anchor's venue keeps the merged bot
    order!(elsewhere, base: 'ETH', exchange: kraken)

    merge = Bot::Merge.new(@user, [anchor.id, elsewhere.id])
    assert_nil merge.reason
    assert_equal @exchange, merge.exchange
    assert_equal [kraken], merge.other_exchanges

    merged = merge!(anchor, elsewhere)

    assert_equal @exchange, merged.exchange
    assert_equal({ @btc.id.to_s => 0.5, @eth.id.to_s => 0.5 }, merged.allocations)
    assert_equal 1, merged.transactions.count
    assert_equal kraken.id, merged.transactions.sole.exchange_id, 'the order keeps the venue it was placed on'
    assert_predicate elsewhere.reload, :deleted?
  end

  test 'when the anchor venue lacks a member, the merged bot goes to a connected venue that has all, else any' do
    kraken = create(:kraken_exchange)
    coinbase = create(:coinbase_exchange)
    anchor = basket([@btc])
    on_kraken = create(:dca_multi_asset, user: @user, exchange: kraken, quote_asset: @usd, base_assets: [@sol],
                                         status: :stopped, with_api_key: false)
    # Kraken lists BTC/USD as well, Binance does not list SOL/USD: Kraken is the only common venue.
    create(:ticker, exchange: kraken, base_asset: @btc, quote_asset: @usd)

    merge = Bot::Merge.new(@user, [anchor.id, on_kraken.id])
    assert_nil merge.reason
    assert_equal kraken, merge.exchange

    # A connected venue that lists everything wins over one that is not connected.
    create(:ticker, exchange: coinbase, base_asset: @btc, quote_asset: @usd)
    create(:ticker, exchange: coinbase, base_asset: @sol, quote_asset: @usd)
    create(:api_key, user: @user, exchange: coinbase)
    merge = Bot::Merge.new(@user, [anchor.id, on_kraken.id])
    assert_equal coinbase, merge.exchange

    merged = merge.perform!
    assert merged, merge.error
    assert_equal coinbase, merged.exchange
    assert_equal 2, merged.bot_index_assets.in_index.count
    assert_equal coinbase.id, merged.bot_index_assets.in_index.first.ticker.exchange_id
  end

  test 'no venue listing every asset together is a refusal, and a member listed nowhere is simply dropped' do
    kraken = create(:kraken_exchange)
    anchor = basket([@btc])
    on_kraken = create(:dca_multi_asset, user: @user, exchange: kraken, quote_asset: @usd, base_assets: [@sol],
                                         status: :stopped, with_api_key: false)

    merge = Bot::Merge.new(@user, [anchor.id, on_kraken.id])
    assert_equal :no_common_exchange, merge.reason
    assert_nil merge.perform!
    assert_equal I18n.t('errors.bots.merge.no_common_exchange'), merge.error

    # SOL delisted everywhere: it no longer counts, BTC alone decides, and SOL arrives as a holding.
    Ticker.where(base_asset: @sol).update_all(available: false)
    merge = Bot::Merge.new(@user, [anchor.id, on_kraken.id])
    assert_nil merge.reason
    assert_equal @exchange, merge.exchange
    merged = merge.perform!
    assert merged, merge.error
    assert_equal [@btc.id], merged.base_asset_ids
    assert_not merged.bot_index_assets.find_by(asset_id: @sol.id).in_index?
  end

  test 'a source with orders resting on a venue the merged bot leaves cannot join' do
    kraken = create(:kraken_exchange)
    anchor = basket([@btc])
    on_kraken = create(:dca_multi_asset, user: @user, exchange: kraken, quote_asset: @usd, base_assets: [@eth],
                                         status: :stopped, with_api_key: false)
    ticker_for(@eth)
    create(:transaction, :open, bot: on_kraken, exchange: kraken, base: 'ETH', quote: 'USD')

    merge = Bot::Merge.new(@user, [anchor.id, on_kraken.id])

    assert_equal [:open_orders, { label: on_kraken.label, exchange: kraken.name }], merge.reason
    assert_nil merge.perform!
    assert_predicate on_kraken.reload, :stopped?
  end

  # == Picking the venue ==

  test 'every venue that lists all the members is offered, and the one picked is where the bot lands' do
    kraken = create(:kraken_exchange)
    coinbase = create(:coinbase_exchange)
    anchor = basket([@btc])
    other = basket([@eth])
    create(:ticker, exchange: kraken, base_asset: @btc, quote_asset: @usd)
    create(:ticker, exchange: kraken, base_asset: @eth, quote_asset: @usd)
    create(:ticker, exchange: coinbase, base_asset: @btc, quote_asset: @usd) # no ETH: not offered

    merge = Bot::Merge.new(@user, [anchor.id, other.id])
    assert_equal @exchange, merge.exchange, 'nothing picked: the venue chosen for the user, as before'
    assert_equal [@exchange, kraken], merge.exchanges

    merge = Bot::Merge.new(@user, [anchor.id, other.id], exchange_id: kraken.id)
    assert_nil merge.reason
    assert_equal kraken, merge.exchange
    assert_equal [@exchange], merge.other_exchanges

    merged = merge.perform!
    assert merged, merge.error
    assert_equal kraken, merged.exchange
    assert_equal [kraken.id], merged.bot_index_assets.in_index.map { it.ticker.exchange_id }.uniq
  end

  test 'a picked venue that does not list every member is refused, never swapped for another' do
    coinbase = create(:coinbase_exchange)
    anchor = basket([@btc])
    other = basket([@eth])
    create(:ticker, exchange: coinbase, base_asset: @btc, quote_asset: @usd)

    merge = Bot::Merge.new(@user, [anchor.id, other.id], exchange_id: coinbase.id)

    assert_equal :no_common_exchange, merge.reason
    assert_no_difference('Bot.count') { assert_nil merge.perform! }
    assert_predicate anchor.reload, :stopped?
  end

  test 'a source with orders resting on its venue keeps the choice to that venue' do
    kraken = create(:kraken_exchange)
    anchor = basket([@btc])
    other = basket([@eth])
    create(:ticker, exchange: kraken, base_asset: @btc, quote_asset: @usd)
    create(:ticker, exchange: kraken, base_asset: @eth, quote_asset: @usd)
    create(:transaction, :open, bot: other, exchange: @exchange, base: 'ETH', quote: 'USD')

    assert_equal [@exchange], Bot::Merge.new(@user, [anchor.id, other.id]).exchanges

    merge = Bot::Merge.new(@user, [anchor.id, other.id], exchange_id: kraken.id)
    assert_equal [:open_orders, { label: other.label, exchange: @exchange.name }], merge.reason

    # Resting orders on two venues: no venue can keep both.
    on_kraken = create(:dca_multi_asset, user: @user, exchange: kraken, quote_asset: @usd, base_assets: [@btc],
                                         status: :stopped, with_api_key: false)
    create(:transaction, :open, bot: on_kraken, exchange: kraken, base: 'BTC', quote: 'USD')
    assert_empty Bot::Merge.new(@user, [on_kraken.id, other.id]).exchanges
  end

  test 'a source moved to another venue after the leases were taken is refused, and the leases handed back' do
    kraken = create(:kraken_exchange)
    anchor = basket([@btc])
    other = basket([@eth])
    create(:ticker, exchange: kraken, base_asset: @eth, quote_asset: @usd)
    moved = Class.new(Bot::Merge) do
      define_method(:load_bots) do |lock: false|
        Bot.where(id: bots.last.id).update_all(exchange_id: kraken.id) if lock # between the leases and the lock
        super(lock:)
      end
    end

    merge = moved.new(@user, [anchor.id, other.id])
    assert_no_difference 'Bot.count' do
      assert_nil merge.perform!
    end

    assert_equal I18n.t('errors.bots.merge.unavailable', label: anchor.label), merge.error
    assert_equal 1, SolidQueue::Semaphore.find_by(key: Bot::Merge::ExchangeLease.for(@exchange).concurrency_key).value
    assert_nil SolidQueue::Semaphore.find_by(key: Bot::Merge::ExchangeLease.for(kraken).concurrency_key)
  end

  test 'a poll queued for an inherited order placed on another venue asks nothing' do
    kraken = create(:kraken_exchange)
    anchor = basket([@btc])
    on_kraken = create(:dca_multi_asset, user: @user, exchange: kraken, quote_asset: @usd, base_assets: [@eth],
                                         status: :stopped, with_api_key: false)
    ticker_for(@eth)
    inherited = order!(on_kraken, base: 'ETH', exchange: kraken)
    merged = merge!(anchor, on_kraken)
    assert_equal merged.id, inherited.reload.bot_id
    Bots::DcaMultiAsset.any_instance.expects(:get_order).never

    Bot::FetchAndUpdateOrderJob.new.perform(inherited)

    assert_predicate inherited.reload, :closed?
  end

  test 'an accepted order id equal to an inherited row from another venue is refused, not recorded against it' do
    kraken = create(:kraken_exchange)
    anchor = basket([@btc])
    on_kraken = create(:dca_multi_asset, user: @user, exchange: kraken, quote_asset: @usd, base_assets: [@eth],
                                         status: :stopped, with_api_key: false)
    ticker_for(@eth)
    order!(on_kraken, base: 'ETH', exchange: kraken, external_id: 'shared-id')
    merged = merge!(anchor, on_kraken)
    ticker = Ticker.find_by!(exchange: @exchange, base_asset: @btc, quote_asset: @usd)
    order_data = { ticker:, side: :buy, order_type: :market_order, price: 100, amount: 1, quote_amount: 100, status: :open }

    assert_raises(ActiveRecord::RecordNotUnique) { merged.persist_accepted_order!(order_data, 'shared-id') }
    assert_equal kraken.id, merged.transactions.find_by(external_id: 'shared-id').exchange_id, 'the inherited row is untouched'
  end

  test 'a merged bot can sell in the name of an inherited holding only what it bought on its own venue' do
    kraken = create(:kraken_exchange)
    anchor = basket([@btc])
    on_kraken = create(:dca_multi_asset, user: @user, exchange: kraken, quote_asset: @usd, base_assets: [@eth],
                                         status: :stopped, with_api_key: false)
    ticker_for(@eth)
    order!(anchor, base: 'BTC', amount: 1, amount_exec: 1)
    order!(on_kraken, base: 'ETH', exchange: kraken, amount: 2, amount_exec: 2)
    merged = merge!(anchor, on_kraken)
    order!(merged, base: 'ETH', amount: 0.5, amount_exec: 0.5) # bought here, after the merge
    order!(merged, base: 'ETH', amount: 0.1, amount_exec: 0.1, side: :sell)
    merged.stubs(:get_balance).returns(Result::Success.new({ free: 5 }))

    assert_equal 0.4, merged.send(:live_free_balance, @eth.id), 'two Kraken ETH are not on Binance'
    assert_equal 5, merged.send(:live_free_balance, @btc.id), 'BTC has no foreign rows: the account as before'
    assert_equal 5, merged.send(:live_free_balance, @usd.id), 'cash is never capped'

    plain = basket([@sol])
    plain.stubs(:get_balance).returns(Result::Success.new({ free: 5 }))
    assert_equal 5, plain.send(:live_free_balance, @sol.id), 'a single-venue history reads the account as before'
  end

  test 'a tick running on any source venue refuses the merge' do
    kraken = create(:kraken_exchange)
    anchor = basket([@btc])
    on_kraken = create(:dca_multi_asset, user: @user, exchange: kraken, quote_asset: @usd, base_assets: [@eth],
                                         status: :stopped, with_api_key: false)
    ticker_for(@eth)
    SolidQueue::Semaphore.create!(key: Bot::Merge::ExchangeLease.for(kraken).concurrency_key, value: 0,
                                  expires_at: 5.minutes.from_now)

    merge = Bot::Merge.new(@user, [anchor.id, on_kraken.id])
    assert_nil merge.perform!

    assert_equal I18n.t('errors.bots.merge.unavailable', label: anchor.label), merge.error
    assert_equal 1, SolidQueue::Semaphore.find_by(key: Bot::Merge::ExchangeLease.for(@exchange).concurrency_key).value,
                 'the lease taken on the anchor venue was handed straight back'
  end

  test 'a signal bot, an archived bot, an executing bot, a bot mid-rebalance and an index bot without rows are unavailable' do
    anchor = basket([@btc])
    signal = create(:signal_bot, user: @user, exchange: @exchange, base_asset: @eth, quote_asset: @usd)
    archived = basket([@eth], status: :archived)
    executing = basket([@eth], status: :executing)
    rebalancing = basket([@eth])
    rebalancing.merge_transient_data!(Bot::Rebalanceable::PENDING_KEY => { phase: 'selling' })
    bare_index = index_bot

    [signal, archived, executing, rebalancing, bare_index].each do |bot|
      reason = Bot::Merge.new(@user, [anchor.id, bot.id]).reason
      assert_equal [:unavailable, { label: bot.label }], reason, "#{bot.type} #{bot.status} should be unavailable"
      assert_not Bot::Merge.mergeable?(bot)
    end
    assert Bot::Merge.mergeable?(anchor)
  end

  test 'a metrics job in the queue is not a reason to refuse' do
    anchor = basket([@btc])
    other = basket([@eth])
    SolidQueue::Job.create!(queue_name: 'default', class_name: 'Bot::UpdateMetricsJob', priority: 0,
                            arguments: { 'job_class' => 'Bot::UpdateMetricsJob',
                                         'arguments' => [{ '_aj_globalid' => other.to_global_id.to_s }] })

    assert merge!(anchor, other)
  end

  # == Over the cap ==

  test 'a merge past the cap is saved, cannot start, and can be trimmed back under it' do
    extra = Array.new(55) { |i| create(:asset, symbol: "X#{i}", name: "Extra #{i}", external_id: "extra-#{i}") }
    anchor = basket(extra.first(53))
    other = basket(extra.drop(53) + [@btc, @eth, @sol])

    merged = merge!(anchor, other)

    assert_equal 58, merged.base_asset_ids.size
    assert_equal 0, merged.excess_members
    assert merged.valid?(:start), merged.errors.full_messages.to_sentence

    wide = Array.new(45) { |i| create(:asset, symbol: "Y#{i}", name: "Wide #{i}", external_id: "wide-#{i}") }
    wider = merge!(merged.reload, basket(wide))

    assert_equal 103, wider.base_asset_ids.size
    assert_equal 3, wider.excess_members
    assert_not wider.valid?(:start)
    assert_equal ['Too many assets. Remove 3.'], wider.errors[:base]
    assert_predicate wider, :persisted?

    # Each removal on the way back under the cap saves, as the settings page's Remove does it.
    wide.first(3).each_with_index do |asset, index|
      wider.settings = wider.settings.merge(wider.parse_params(remove_asset_id: asset.id.to_s).stringify_keys)
      wider.set_missed_quote_amount
      assert wider.save, wider.errors.full_messages.to_sentence
      assert_equal 2 - index, wider.reload.excess_members
    end
    assert_equal 0, wider.excess_members
    assert_not wider.valid?(:start), 'the remaining weights no longer add up'
    assert_includes wider.errors[:allocations].to_sentence, I18n.t('bot.dca_multi_asset.normalize_first')

    wider.allocations = wider.normalize_allocations(wider.allocations)
    wider.set_missed_quote_amount
    wider.save!
    assert wider.valid?(:start), wider.errors.full_messages.to_sentence
  end

  private

  def merge!(*bots)
    merge = Bot::Merge.new(@user, bots.map(&:id))
    merged = merge.perform!
    assert merged, merge.error
    merged
  end

  def basket(assets, allocations: nil, **attrs)
    create(:dca_multi_asset, user: @user, exchange: @exchange, quote_asset: @usd, base_assets: assets,
                             allocations: allocations, status: :stopped, **attrs)
  end

  def index_bot(**attrs)
    create(:dca_index, user: @user, exchange: @exchange, quote_asset: @usd, status: :stopped, **attrs)
  end

  def ticker_for(asset)
    Ticker.find_by(exchange: @exchange, base_asset: asset, quote_asset: @usd) ||
      create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: @usd)
  end

  def membership!(bot, asset, weight, in_index: true, entered_at: 1.week.ago, exited_at: nil)
    BotIndexAsset.create!(bot:, asset:, ticker: ticker_for(asset), in_index:, target_allocation: weight,
                          entered_at:, exited_at:)
  end

  def write_settings!(bot, **pairs)
    bot.update_columns(settings: bot.settings.merge(pairs.stringify_keys))
    bot.reload
  end

  def order!(bot, base:, **columns)
    # The venue defaults to the anchor's; a column given explicitly (exchange: kraken) wins.
    create(:transaction, exchange: @exchange, bot:, base:, quote: 'USD', status: :submitted, external_status: :closed,
                         side: :buy, external_id: "o-#{SecureRandom.hex(4)}", price: 100, amount: 1, amount_exec: 1,
                         quote_amount: 100, quote_amount_exec: 100, **columns)
  end

  # A queued Bot::ActionJob addressed to the bot, in a given execution state — built by hand so the
  # test controls exactly which Solid Queue rows exist.
  def enqueue_job_for(bot, state: :none, scheduled_at: 1.hour.from_now)
    job = SolidQueue::Job.create!(
      queue_name: 'default', class_name: 'Bot::ActionJob', priority: 0,
      arguments: { 'job_class' => 'Bot::ActionJob',
                   'arguments' => [{ '_aj_globalid' => "gid://deltabadger/#{bot.type}/#{bot.id}" }] }
    )
    SolidQueue::ReadyExecution.where(job_id: job.id).delete_all unless state == :ready
    case state
    when :blocked
      job.update!(concurrency_key: Bot::Merge::ExchangeLease.for(bot.exchange).concurrency_key)
      SolidQueue::BlockedExecution.create!(job_id: job.id, queue_name: job.queue_name, priority: job.priority,
                                           concurrency_key: job.concurrency_key, expires_at: 5.minutes.from_now)
    when :scheduled
      job.update!(scheduled_at:)
      SolidQueue::ScheduledExecution.create!(job_id: job.id, queue_name: job.queue_name, priority: job.priority,
                                             scheduled_at:)
    end
    job
  end
end
