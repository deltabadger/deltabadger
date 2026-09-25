# frozen_string_literal: true

require 'test_helper'

# A portfolio follows an index, an index bot changes its index or becomes a portfolio: the row switches
# class in place and keeps everything else. What matters is what the new bot is made of, what it keeps,
# what the switch refuses, and that nothing loaded as the old class can write over it afterwards.
class Bot::IndexSwitchTest < ActiveSupport::TestCase
  setup do
    Bot::UpdateMetricsJob.stubs(:perform_later)
    Bot::ResyncIndexCompositionJob.stubs(:perform_later)
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @usd = create(:asset, :usd)
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum)
    @sol = create(:asset, symbol: 'SOL', name: 'Solana', external_id: 'solana')
    # Market caps 3 : 2 : 1, so pure market-cap weights and equal weights differ.
    MarketData.stubs(:get_top_coins).returns(Result::Success.new(
                                               [{ 'id' => 'bitcoin', 'market_cap' => 300 }, { 'id' => 'ethereum', 'market_cap' => 200 },
                                                { 'id' => 'solana', 'market_cap' => 100 }]
                                             ))
    Ticker.any_instance.stubs(:priced?).returns(true)
    @top = Index.create!(external_id: Index::TOP_COINS_EXTERNAL_ID, source: Index::SOURCE_INTERNAL, name: 'Top Coins',
                         top_coins: %w[bitcoin ethereum solana])
  end

  # == Follow index ==

  test 'a portfolio follows an index in place: same row and history, the index settings, none of the basket' do
    bot = basket([@btc, @eth])
    order!(bot, base: 'BTC')
    Bot::ResyncIndexCompositionJob.unstub(:perform_later)
    Bot::ResyncIndexCompositionJob.expects(:perform_later).once

    switched = Bot::IndexSwitch.follow!(bot, @top)

    assert_instance_of Bots::DcaIndex, switched
    assert_equal bot.id, switched.id
    assert_equal 1, switched.transactions.count
    assert_equal Bots::DcaIndex::INDEX_TYPE_TOP, switched.index_type
    assert_equal 10, switched.num_coins.to_i
    assert_not switched.hold_all?
    assert_equal 0.0, switched.allocation_flattening.to_f
    assert_equal 100.0, switched.quote_amount.to_f
    %w[allocations weighting direction].each { |key| assert_not switched.settings.key?(key), "#{key} leaked" }
    assert_equal 'Top 10', switched.label, 'a name the user never changed follows the new composition'
  end

  test 'the new members are derived before the switch returns, so the page shows them at once' do
    bot = basket([@btc, @eth])
    layer1 = Index.create!(external_id: 'layer-1', source: Index::SOURCE_COINGECKO, name: 'Layer 1',
                           top_coins: %w[solana])
    stub_top_coins('solana' => 100)
    ticker_for(@sol)

    switched = Bot::IndexSwitch.follow!(bot, layer1)

    assert_equal [@sol.id], switched.bot_index_assets.in_index.pluck(:asset_id)
    assert_equal [@btc.id, @eth.id].sort, switched.bot_index_assets.where(in_index: false).pluck(:asset_id).sort
  end

  test 'a bounded index is held whole, like a new bot on it' do
    nd = Index.create!(external_id: 'nasdaq-100', source: Index::SOURCE_DELTABADGER, name: 'Nasdaq 100',
                       top_coins: %w[bitcoin ethereum solana])

    switched = Bot::IndexSwitch.follow!(basket([@btc]), nd)

    assert_equal 'nasdaq-100', switched.index_category_id
    assert_equal 'ND', switched.index_name_prefix
    assert switched.hold_all?
    assert_equal 3, switched.num_coins.to_i
  end

  test 'a renamed bot keeps its name' do
    bot = basket([@btc])
    bot.update_columns(label: 'Retirement')

    assert_equal 'Retirement', Bot::IndexSwitch.follow!(bot, @top).label
  end

  test 'an index bot changes its index even after it has bought' do
    bot = index_bot
    order!(bot, base: 'BTC')
    layer1 = Index.create!(external_id: 'layer-1', source: Index::SOURCE_COINGECKO, name: 'Layer 1',
                           top_coins: %w[bitcoin ethereum solana])

    switched = Bot::IndexSwitch.follow!(bot, layer1)

    assert_instance_of Bots::DcaIndex, switched
    assert_equal 'layer-1', switched.index_category_id
    assert_equal 'Layer 1', switched.index_name
  end

  test 'a selling portfolio cannot follow an index' do
    bot = basket([@btc])
    write_settings!(bot, direction: 'selling')

    error = assert_raises(Bot::IndexSwitch::Refused) { Bot::IndexSwitch.follow!(bot, @top) }
    assert_equal I18n.t('errors.bots.index_switch.selling'), error.message
    assert_instance_of Bots::DcaMultiAsset, Bot.find(bot.id)
  end

  # == Custom allocation ==

  test 'an index bot becomes a portfolio of its current members at their weights' do
    stub_top_coins('bitcoin' => 300, 'ethereum' => 200)
    bot = index_bot
    membership!(bot, @btc, 0.6)
    membership!(bot, @eth, 0.4)
    membership!(bot, @sol, 0.0, in_index: false, exited_at: 1.day.ago)

    switched = Bot::IndexSwitch.customize!(bot)

    assert_instance_of Bots::DcaMultiAsset, switched
    assert_equal({ @btc.id.to_s => 0.6, @eth.id.to_s => 0.4 }, switched.allocations)
    %w[num_coins index_type index_category_id allocation_flattening hold_all].each do |key|
      assert_not switched.settings.key?(key), "#{key} leaked"
    end
    assert_not switched.bot_index_assets.find_by(asset_id: @sol.id).in_index, 'an exited member stays exited'
    assert_equal 'BTC, ETH', switched.label
  end

  test 'the portfolio keeps the weights the flattening slider set, even before a resync ran' do
    bot = index_bot
    bot.update_columns(settings: bot.settings.merge('num_coins' => 3, 'allocation_flattening' => 0.0))
    Bot.find(bot.id).refresh_composition # market-cap targets 1/2 : 1/3 : 1/6 on record
    bot.update_columns(settings: bot.settings.merge('allocation_flattening' => 1.0)) # slider moved, not re-derived

    switched = Bot::IndexSwitch.customize!(Bot.find(bot.id))

    assert_equal [0.333, 0.333, 0.334], switched.allocations.values.sort
  end

  test 'moving the flattening slider re-derives the stored weights' do
    bot = index_bot
    Bot::ResyncIndexCompositionJob.unstub(:perform_later)
    Bot::ResyncIndexCompositionJob.expects(:perform_later).once

    bot.set_missed_quote_amount
    bot.update!(allocation_flattening: 1.0)
  end

  test 'a member the venue no longer trades is left out of the portfolio' do
    stub_top_coins('bitcoin' => 100, 'ethereum' => 100)
    bot = index_bot
    membership!(bot, @btc, 0.5)
    membership!(bot, @eth, 0.5)
    ticker_for(@eth).update_columns(available: false)

    switched = Bot::IndexSwitch.customize!(bot)

    assert_equal({ @btc.id.to_s => 1.0 }, switched.allocations)
    assert_not switched.bot_index_assets.find_by(asset_id: @eth.id).in_index
  end

  # == Refusals ==

  test 'a running bot is refused' do
    bot = basket([@btc], status: :scheduled, started_at: 1.day.ago)

    error = assert_raises(Bot::IndexSwitch::Refused) { Bot::IndexSwitch.follow!(bot, @top) }
    assert_equal I18n.t('errors.bots.index_switch.stop_first'), error.message
  end

  test 'a pending swap is refused' do
    bot = index_bot
    membership!(bot, @btc, 1.0)
    bot.merge_transient_data!(Bot::Rebalanceable::PENDING_KEY => { 'phase' => 'selling' })

    assert_raises(Bot::IndexSwitch::Refused) { Bot::IndexSwitch.customize!(bot) }
    assert_instance_of Bots::DcaIndex, Bot.find(bot.id)
  end

  test 'a bot started after the request loaded it is refused inside the transaction' do
    bot = basket([@btc])
    Bot.where(id: bot.id).update_all(status: Bot.statuses[:scheduled])

    assert_raises(Bot::IndexSwitch::Refused) { Bot::IndexSwitch.follow!(bot, @top) }
    assert_instance_of Bots::DcaMultiAsset, Bot.find(bot.id)
  end

  test 'a trading job holding the venue lease refuses the switch' do
    bot = basket([@btc])
    SolidQueue::Semaphore.create!(key: Bot::VenueLease::ExchangeLease.for(@exchange).concurrency_key, value: 0,
                                  expires_at: 5.minutes.from_now)

    error = assert_raises(Bot::IndexSwitch::Refused) { Bot::IndexSwitch.follow!(bot, @top) }
    assert_equal I18n.t('errors.bots.index_switch.in_flight'), error.message
    assert_instance_of Bots::DcaMultiAsset, Bot.find(bot.id)
  end

  test 'the page broadcasts naming the bot do not hold the switch up' do
    bot = basket([@btc])
    broadcast = enqueue_job_for(bot, 'Bot::BroadcastMetricsUpdateJob')
    SolidQueue::ScheduledExecution.where(job_id: broadcast.id).delete_all
    SolidQueue::ReadyExecution.create!(job_id: broadcast.id, queue_name: broadcast.queue_name, priority: broadcast.priority)

    assert_instance_of Bots::DcaIndex, Bot::IndexSwitch.follow!(bot, @top)
  end

  test 'a condition poll that is already running is refused' do
    bot = basket([@btc])
    poll = enqueue_job_for(bot, 'Bot::PriceLimitCheckJob')
    SolidQueue::ScheduledExecution.where(job_id: poll.id).delete_all
    SolidQueue::ReadyExecution.create!(job_id: poll.id, queue_name: poll.queue_name, priority: poll.priority)

    error = assert_raises(Bot::IndexSwitch::Refused) { Bot::IndexSwitch.follow!(bot, @top) }
    assert_equal I18n.t('errors.bots.index_switch.in_flight'), error.message
  end

  test 'a dead-lettered poll does not hold the switch up forever' do
    bot = basket([@btc])
    poll = enqueue_job_for(bot, 'Bot::PriceLimitCheckJob')
    SolidQueue::ScheduledExecution.where(job_id: poll.id).delete_all
    SolidQueue::FailedExecution.create!(job_id: poll.id, error: 'boom')

    assert_instance_of Bots::DcaIndex, Bot::IndexSwitch.follow!(bot, @top)
  end

  test 'a bot moved to another exchange since the request loaded it is refused' do
    bot = basket([@btc])
    kraken = create(:kraken_exchange)
    Bot.where(id: bot.id).update_all(exchange_id: kraken.id)

    assert_raises(Bot::IndexSwitch::Refused) { Bot::IndexSwitch.follow!(bot, @top) }
    assert_instance_of Bots::DcaMultiAsset, Bot.find(bot.id)
  end

  # == Stale instances and queued jobs ==

  test 'an instance loaded as the old class can neither save, start, nor write members afterwards' do
    bot = index_bot
    membership!(bot, @btc, 0.5)
    membership!(bot, @eth, 0.5)
    stale = Bot.find(bot.id)

    Bot::IndexSwitch.customize!(bot)

    stale.set_missed_quote_amount # what every settings save does first (BotsController#update)
    assert_raises(ActiveRecord::StaleObjectError) { stale.update!(label: 'Overwritten') }
    assert_not stale.start, 'a start from the old class arms nothing'
    stale.send(:update_bot_index_assets, [{ asset_id: doge.id, ticker_id: ticker_for(doge).id, weight: 1.0 }])
    assert_not BotIndexAsset.exists?(bot_id: bot.id, asset_id: doge.id), 'a stale derivation writes no members'
    assert_instance_of Bots::DcaMultiAsset, Bot.find(bot.id)
  end

  test 'a derivation from settings replaced since it loaded writes no members' do
    bot = index_bot
    membership!(bot, @btc, 1.0)
    stale = Bot.find(bot.id)
    layer1 = Index.create!(external_id: 'layer-1', source: Index::SOURCE_COINGECKO, name: 'Layer 1',
                           top_coins: %w[bitcoin ethereum solana])

    Bot::IndexSwitch.follow!(bot, layer1)
    stale.send(:update_bot_index_assets, [{ asset_id: doge.id, ticker_id: ticker_for(doge).id, weight: 1.0 }])

    assert_not BotIndexAsset.exists?(bot_id: bot.id, asset_id: doge.id)
  end

  test 'a future job is repointed to the new class, and the portfolio condition polls are dropped' do
    bot = basket([@btc])
    tick = enqueue_job_for(bot, 'Bot::ActionJob')
    poll = enqueue_job_for(bot, 'Bot::PriceLimitCheckJob')

    Bot::IndexSwitch.follow!(bot, @top)

    assert_includes tick.reload.arguments.to_s, "Bots::DcaIndex/#{bot.id}"
    assert_not SolidQueue::Job.exists?(poll.id)
  end

  private

  def doge
    @doge ||= create(:asset, symbol: 'DOGE', name: 'Dogecoin', external_id: 'dogecoin')
  end

  def stub_top_coins(caps)
    MarketData.stubs(:get_top_coins).returns(Result::Success.new(caps.map { |id, cap| { 'id' => id, 'market_cap' => cap } }))
  end

  def basket(assets, **attrs)
    create(:dca_multi_asset, user: @user, exchange: @exchange, quote_asset: @usd, base_assets: assets,
                             status: :stopped, **attrs)
  end

  def index_bot(**attrs)
    bot = create(:dca_index, user: @user, exchange: @exchange, quote_asset: @usd, status: :stopped, **attrs)
    [@btc, @eth, @sol].each { |asset| ticker_for(asset) }
    bot
  end

  def ticker_for(asset)
    Ticker.find_by(exchange: @exchange, base_asset: asset, quote_asset: @usd) ||
      create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: @usd)
  end

  def membership!(bot, asset, weight, in_index: true, exited_at: nil)
    BotIndexAsset.create!(bot:, asset:, ticker: ticker_for(asset), in_index:, target_allocation: weight,
                          entered_at: 1.week.ago, exited_at:)
  end

  def write_settings!(bot, **pairs)
    bot.update_columns(settings: bot.settings.merge(pairs.stringify_keys))
    bot.reload
  end

  def order!(bot, base:)
    create(:transaction, exchange: @exchange, bot:, base:, quote: 'USD', status: :submitted, external_status: :closed,
                         side: :buy, external_id: "o-#{SecureRandom.hex(4)}", price: 100, amount: 1, amount_exec: 1,
                         quote_amount: 100, quote_amount_exec: 100)
  end

  def enqueue_job_for(bot, class_name)
    job = SolidQueue::Job.create!(
      queue_name: 'default', class_name:, priority: 0, scheduled_at: 1.hour.from_now,
      arguments: { 'job_class' => class_name,
                   'arguments' => [{ '_aj_globalid' => "gid://deltabadger/#{bot.type}/#{bot.id}" }] }
    )
    SolidQueue::ReadyExecution.where(job_id: job.id).delete_all
    SolidQueue::ScheduledExecution.find_or_create_by!(job_id: job.id) do |execution|
      execution.assign_attributes(queue_name: job.queue_name, priority: job.priority, scheduled_at: job.scheduled_at)
    end
    job
  end
end
