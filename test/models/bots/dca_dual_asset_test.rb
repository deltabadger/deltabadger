require 'test_helper'

class Bots::DcaDualAssetTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

  # == Associations ==

  test 'belongs to exchange (optional)' do
    bot = create(:dca_dual_asset)
    assert_respond_to bot, :exchange
    assert_kind_of Exchange, bot.exchange

    bot.exchange = nil
    assert_predicate bot, :valid?
  end

  test 'belongs to user' do
    bot = create(:dca_dual_asset)
    assert_respond_to bot, :user
    assert_kind_of User, bot.user
  end

  test 'has many transactions' do
    bot = create(:dca_dual_asset)
    assert_respond_to bot, :transactions
    assert_kind_of ActiveRecord::Associations::CollectionProxy, bot.transactions

    transaction = create(:transaction, bot: bot)
    assert_equal 1, bot.transactions.count
    assert_includes bot.transactions, transaction
  end

  # == Validations: quote_amount ==

  test 'requires quote_amount to be present' do
    bot = build(:dca_dual_asset)
    bot.quote_amount = nil
    assert_not_predicate bot, :valid?
    assert_includes bot.errors[:quote_amount], "can't be blank"
  end

  test 'requires quote_amount to be greater than 0' do
    bot = build(:dca_dual_asset)
    bot.quote_amount = 0
    assert_not_predicate bot, :valid?
    assert bot.errors[:quote_amount].present?
  end

  test 'accepts positive quote_amount' do
    bot = build(:dca_dual_asset)
    bot.quote_amount = 100
    assert bot.errors[:quote_amount].empty?
  end

  # == Validations: allocation0 ==

  test 'requires allocation0 to be present' do
    bot = build(:dca_dual_asset)
    bot.allocation0 = nil
    assert_not_predicate bot, :valid?
    assert_includes bot.errors[:allocation0], "can't be blank"
  end

  test 'requires allocation0 to be at least 0' do
    bot = build(:dca_dual_asset)
    bot.allocation0 = -0.1
    assert_not_predicate bot, :valid?
    assert bot.errors[:allocation0].present?
  end

  test 'requires allocation0 to be at most 1' do
    bot = build(:dca_dual_asset)
    bot.allocation0 = 1.1
    assert_not_predicate bot, :valid?
    assert bot.errors[:allocation0].present?
  end

  test 'accepts allocation0 between 0 and 1' do
    bot = build(:dca_dual_asset)
    [0, 0.3, 0.5, 0.7, 1].each do |value|
      bot.allocation0 = value
      assert bot.errors[:allocation0].empty?
    end
  end

  # == Validations: interval ==

  test 'requires interval to be present' do
    bot = build(:dca_dual_asset)
    bot.interval = nil
    assert_not_predicate bot, :valid?
    assert_includes bot.errors[:interval], "can't be blank"
  end

  test 'accepts valid interval values' do
    bot = build(:dca_dual_asset)
    %w[hour day week month].each do |interval|
      bot.interval = interval
      assert bot.errors[:interval].empty?
    end
  end

  test 'rejects invalid interval values' do
    bot = build(:dca_dual_asset)
    bot.interval = 'minute'
    assert_not_predicate bot, :valid?
    assert_includes bot.errors[:interval], 'is not included in the list'
  end

  # == Validations: validate_external_ids ==

  test 'is valid when all three assets exist' do
    bot = create(:dca_dual_asset)
    assert bot.valid?(:update)
  end

  test 'is invalid when base0_asset does not exist' do
    bot = create(:dca_dual_asset)
    bot.base0_asset_id = 999_999
    assert_not bot.valid?(:update)
    assert_includes bot.errors[:base0_asset_id], 'is invalid'
  end

  test 'is invalid when base1_asset does not exist' do
    bot = create(:dca_dual_asset)
    bot.base1_asset_id = 999_999
    assert_not bot.valid?(:update)
    assert_includes bot.errors[:base1_asset_id], 'is invalid'
  end

  test 'is invalid when quote_asset does not exist' do
    bot = create(:dca_dual_asset)
    bot.quote_asset_id = 999_999
    assert_not bot.valid?(:update)
    assert_includes bot.errors[:quote_asset_id], 'is invalid'
  end

  # == Validations: validate_bot_exchange ==

  test 'is valid when exchange supports both asset pairs' do
    exchange = create(:binance_exchange)
    bitcoin = create(:asset, :bitcoin)
    ethereum = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    create(:ticker, exchange: exchange, base_asset: bitcoin, quote_asset: usd)
    create(:ticker, exchange: exchange, base_asset: ethereum, quote_asset: usd)
    bot = create(:dca_dual_asset, exchange: exchange, base0_asset: bitcoin, base1_asset: ethereum, quote_asset: usd)

    assert bot.valid?(:update)
  end

  test 'is invalid when exchange does not support base0 pair' do
    exchange = create(:binance_exchange)
    bitcoin = create(:asset, :bitcoin)
    ethereum = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    solana = create(:asset, external_id: 'solana', symbol: 'SOL', name: 'Solana')
    create(:ticker, exchange: exchange, base_asset: bitcoin, quote_asset: usd)
    create(:ticker, exchange: exchange, base_asset: ethereum, quote_asset: usd)
    bot = create(:dca_dual_asset, exchange: exchange, base0_asset: bitcoin, base1_asset: ethereum, quote_asset: usd)

    bot.set_missed_quote_amount
    bot.base0_asset_id = solana.id
    assert_not bot.valid?(:update)
    assert bot.errors[:exchange].present?
  end

  test 'is invalid when exchange does not support base1 pair' do
    exchange = create(:binance_exchange)
    bitcoin = create(:asset, :bitcoin)
    ethereum = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    solana = create(:asset, external_id: 'solana', symbol: 'SOL', name: 'Solana')
    create(:ticker, exchange: exchange, base_asset: bitcoin, quote_asset: usd)
    create(:ticker, exchange: exchange, base_asset: ethereum, quote_asset: usd)
    bot = create(:dca_dual_asset, exchange: exchange, base0_asset: bitcoin, base1_asset: ethereum, quote_asset: usd)

    bot.set_missed_quote_amount
    bot.base1_asset_id = solana.id
    assert_not bot.valid?(:update)
    assert bot.errors[:exchange].present?
  end

  # == Validations: validate_unchangeable_assets ==

  test 'prevents changing base0_asset after transactions exist' do
    bot = create(:dca_dual_asset)
    new_asset = create(:asset, external_id: 'solana', symbol: 'SOL', name: 'Solana')
    create(:transaction, bot: bot)

    bot.set_missed_quote_amount
    bot.base0_asset_id = new_asset.id
    assert_not bot.valid?(:update)
    assert bot.errors[:base0_asset_id].present?
  end

  test 'prevents changing base1_asset after transactions exist' do
    bot = create(:dca_dual_asset)
    new_asset = create(:asset, external_id: 'solana', symbol: 'SOL', name: 'Solana')
    create(:transaction, bot: bot)

    bot.set_missed_quote_amount
    bot.base1_asset_id = new_asset.id
    assert_not bot.valid?(:update)
    assert bot.errors[:base1_asset_id].present?
  end

  test 'prevents changing quote_asset after transactions exist' do
    bot = create(:dca_dual_asset)
    new_asset = create(:asset, external_id: 'solana', symbol: 'SOL', name: 'Solana')
    create(:transaction, bot: bot)

    bot.set_missed_quote_amount
    bot.quote_asset_id = new_asset.id
    assert_not bot.valid?(:update)
    assert bot.errors[:quote_asset_id].present?
  end

  test 'allows changing other settings after transactions exist' do
    bot = create(:dca_dual_asset)
    create(:transaction, bot: bot)

    bot.set_missed_quote_amount
    bot.quote_amount = 200
    bot.allocation0 = 0.7
    assert bot.valid?(:update)
  end

  # == Validations: validate_unchangeable_interval ==

  test 'prevents changing interval while bot is running' do
    bot = create(:dca_dual_asset, :started)
    bot.set_missed_quote_amount
    bot.interval = 'week'
    assert_not bot.valid?(:update)
    assert_includes bot.errors[:settings], 'Interval cannot be changed while the bot is running'
  end

  test 'allows changing interval when bot is stopped' do
    bot = create(:dca_dual_asset, :started)
    bot.status = :stopped
    bot.stopped_at = Time.current
    bot.save!

    bot.set_missed_quote_amount
    bot.interval = 'week'
    assert bot.valid?(:update)
  end

  # == Validations: validate_unchangeable_exchange ==

  test 'prevents changing exchange when there are open orders' do
    bot = create(:dca_dual_asset)
    new_exchange = create(:kraken_exchange)
    create(:transaction, :open, bot: bot)

    create(:ticker, exchange: new_exchange, base_asset: bot.base0_asset, quote_asset: bot.quote_asset)
    create(:ticker, exchange: new_exchange, base_asset: bot.base1_asset, quote_asset: bot.quote_asset)
    create(:api_key, user: bot.user, exchange: new_exchange)

    bot.exchange = new_exchange
    assert_not bot.valid?(:update)
    assert bot.errors[:exchange].present?
  end

  test 'allows changing exchange when there are no open orders' do
    bot = create(:dca_dual_asset)
    new_exchange = create(:kraken_exchange)
    create(:transaction, bot: bot, external_status: :closed)

    create(:ticker, exchange: new_exchange, base_asset: bot.base0_asset, quote_asset: bot.quote_asset)
    create(:ticker, exchange: new_exchange, base_asset: bot.base1_asset, quote_asset: bot.quote_asset)
    create(:api_key, user: bot.user, exchange: new_exchange)

    bot.exchange = new_exchange
    assert bot.valid?(:update)
  end

  # == Validations: validate_tickers_available ==

  test 'is valid on start when both tickers are available' do
    bot = create(:dca_dual_asset)
    assert bot.valid?(:start)
  end

  test 'is invalid on start when ticker0 is not available' do
    bot = create(:dca_dual_asset)
    bot.ticker0.update!(available: false)
    assert_not bot.valid?(:start)
    assert_includes bot.errors[:base0_asset_id], 'is invalid'
  end

  test 'is invalid on start when ticker1 is not available' do
    bot = create(:dca_dual_asset)
    bot.ticker1.update!(available: false)
    assert_not bot.valid?(:start)
    assert_includes bot.errors[:base1_asset_id], 'is invalid'
  end

  # == Settings accessors ==

  test 'provides access to base0_asset_id' do
    bot = create(:dca_dual_asset)
    assert_equal bot.base0_asset.id, bot.base0_asset_id
  end

  test 'provides access to base1_asset_id' do
    bot = create(:dca_dual_asset)
    assert_equal bot.base1_asset.id, bot.base1_asset_id
  end

  test 'provides access to quote_asset_id' do
    bot = create(:dca_dual_asset)
    assert_equal bot.quote_asset.id, bot.quote_asset_id
  end

  test 'provides access to quote_amount' do
    bot = create(:dca_dual_asset)
    assert_equal 100.0, bot.quote_amount
  end

  test 'provides access to allocation0' do
    bot = create(:dca_dual_asset)
    assert_equal 0.5, bot.allocation0
  end

  test 'provides access to interval' do
    bot = create(:dca_dual_asset)
    assert_equal 'day', bot.interval
  end

  # == Lifecycle: #start ==

  test 'start changes status to scheduled' do
    bot = create(:dca_dual_asset)
    Bot::ActionJob.stubs(:perform_later)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    bot.start
    assert_equal 'scheduled', bot.status
  end

  test 'start sets started_at to current time' do
    bot = create(:dca_dual_asset)
    Bot::ActionJob.stubs(:perform_later)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    freeze_time do
      bot.start
      assert_equal Time.current, bot.started_at
    end
  end

  test 'start clears stop_message_key' do
    bot = create(:dca_dual_asset)
    Bot::ActionJob.stubs(:perform_later)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    bot.update!(stop_message_key: 'some_key', status: :stopped)
    bot.start
    assert_nil bot.stop_message_key
  end

  test 'start clears last_action_job_at' do
    bot = create(:dca_dual_asset)
    Bot::ActionJob.stubs(:perform_later)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    bot.last_action_job_at = Time.current
    bot.start
    assert_nil bot.last_action_job_at
  end

  test 'start clears missed_quote_amount' do
    bot = create(:dca_dual_asset)
    Bot::ActionJob.stubs(:perform_later)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    bot.missed_quote_amount = 50
    bot.start
    assert_equal 0, bot.missed_quote_amount
  end

  test 'start schedules Bot::ActionJob immediately' do
    bot = create(:dca_dual_asset)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
    Bot::ActionJob.expects(:perform_later).with(bot)

    bot.start
  end

  test 'start returns true on success' do
    bot = create(:dca_dual_asset)
    Bot::ActionJob.stubs(:perform_later)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    assert_equal true, bot.start
  end

  test 'start with start_fresh false schedules Bot::ActionJob immediately' do
    bot = create(:dca_dual_asset, :stopped)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
    Bot::ActionJob.expects(:perform_later).with(bot)

    bot.last_action_job_at = 1.hour.ago.iso8601
    bot.start(start_fresh: false)
  end

  test 'start with start_fresh false preserves started_at' do
    bot = create(:dca_dual_asset, :stopped)
    Bot::ActionJob.stubs(:perform_later)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    original_started_at = bot.started_at
    bot.last_action_job_at = 1.hour.ago.iso8601
    bot.start(start_fresh: false)
    assert_equal original_started_at, bot.started_at
  end

  test 'start returns false when validation fails' do
    bot = create(:dca_dual_asset)
    Bot::ActionJob.stubs(:perform_later)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)

    bot.ticker0.update!(available: false)
    assert_equal false, bot.start
  end

  test 'start does not schedule jobs when validation fails' do
    bot = create(:dca_dual_asset)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
    Bot::ActionJob.expects(:perform_later).never

    bot.ticker0.update!(available: false)
    bot.start
  end

  # == Lifecycle: #stop ==

  test 'stop changes status to stopped' do
    bot = create(:dca_dual_asset, :started)
    bot.stubs(:cancel_scheduled_action_jobs)

    bot.stop
    assert_equal 'stopped', bot.status
  end

  test 'stop sets stopped_at to current time' do
    bot = create(:dca_dual_asset, :started)
    bot.stubs(:cancel_scheduled_action_jobs)

    freeze_time do
      bot.stop
      assert_equal Time.current, bot.stopped_at
    end
  end

  test 'stop cancels scheduled action jobs' do
    bot = create(:dca_dual_asset, :started)
    bot.expects(:cancel_scheduled_action_jobs)

    bot.stop
  end

  test 'stop stores stop_message_key when provided' do
    bot = create(:dca_dual_asset, :started)
    bot.stubs(:cancel_scheduled_action_jobs)

    bot.stop(stop_message_key: 'manual_stop')
    assert_equal 'manual_stop', bot.stop_message_key
  end

  test 'stop returns true on success' do
    bot = create(:dca_dual_asset, :started)
    bot.stubs(:cancel_scheduled_action_jobs)

    assert_equal true, bot.stop
  end

  # == Lifecycle: #delete ==

  test 'delete changes status to deleted' do
    bot = create(:dca_dual_asset, :started)
    bot.stubs(:cancel_scheduled_action_jobs)

    bot.delete
    assert_equal 'deleted', bot.status
  end

  test 'delete sets stopped_at to current time' do
    bot = create(:dca_dual_asset, :started)
    bot.stubs(:cancel_scheduled_action_jobs)

    freeze_time do
      bot.delete
      assert_equal Time.current, bot.stopped_at
    end
  end

  test 'delete cancels scheduled action jobs' do
    bot = create(:dca_dual_asset, :started)
    bot.expects(:cancel_scheduled_action_jobs)

    bot.delete
  end

  test 'delete returns true on success' do
    bot = create(:dca_dual_asset, :started)
    bot.stubs(:cancel_scheduled_action_jobs)

    assert_equal true, bot.delete
  end

  # == Lifecycle: #execute_action ==

  test 'execute_action sets status to waiting on success' do
    bot = create(:dca_dual_asset, :started)
    setup_bot_execution_mocks(bot)
    Bot::FetchAndCreateOrderJob.stubs(:perform_later)
    Bot::FetchAndUpdateOpenOrdersJob.stubs(:perform_now)
    bot.stubs(:broadcast_below_minimums_warning)
    bot.stubs(:set_orders).returns(Result::Success.new)

    bot.execute_action
    assert_equal 'waiting', bot.reload.status
  end

  test 'execute_action calls set_orders with pending_quote_amount' do
    bot = create(:dca_dual_asset, :started)
    setup_bot_execution_mocks(bot)
    Bot::FetchAndCreateOrderJob.stubs(:perform_later)
    Bot::FetchAndUpdateOpenOrdersJob.stubs(:perform_now)
    bot.stubs(:broadcast_below_minimums_warning)

    pending_amount = bot.pending_quote_amount
    bot.expects(:set_orders).with(
      total_orders_amount_in_quote: pending_amount,
      update_missed_quote_amount: true
    ).returns(Result::Success.new)

    bot.execute_action
  end

  test 'execute_action returns Success on success' do
    bot = create(:dca_dual_asset, :started)
    setup_bot_execution_mocks(bot)
    Bot::FetchAndCreateOrderJob.stubs(:perform_later)
    Bot::FetchAndUpdateOpenOrdersJob.stubs(:perform_now)
    bot.stubs(:broadcast_below_minimums_warning)
    bot.stubs(:set_orders).returns(Result::Success.new)

    result = bot.execute_action
    assert_predicate result, :success?
  end

  test 'execute_action returns failure when set_orders fails' do
    bot = create(:dca_dual_asset, :started)
    setup_bot_execution_mocks(bot)
    Bot::FetchAndCreateOrderJob.stubs(:perform_later)
    Bot::FetchAndUpdateOpenOrdersJob.stubs(:perform_now)
    bot.stubs(:broadcast_below_minimums_warning)
    bot.stubs(:set_orders).returns(Result::Failure.new('Order failed'))

    result = bot.execute_action
    assert_predicate result, :failure?
  end

  # == Query methods ==

  test 'restarting? returns true when stopped with last_action_job_at' do
    bot = create(:dca_dual_asset, :stopped)
    bot.last_action_job_at = 1.hour.ago.iso8601
    assert_predicate bot, :restarting?
  end

  test 'restarting? returns false when not stopped' do
    bot = create(:dca_dual_asset, :stopped)
    bot.status = :scheduled
    bot.last_action_job_at = 1.hour.ago.iso8601
    assert_not_predicate bot, :restarting?
  end

  test 'restarting? returns false when stopped but no last_action_job_at' do
    bot = create(:dca_dual_asset, :stopped)
    bot.last_action_job_at = nil
    assert_not_predicate bot, :restarting?
  end

  test 'restarting_within_interval? returns false when not restarting' do
    bot = create(:dca_dual_asset, :stopped)
    bot.status = :scheduled
    bot.last_action_job_at = 1.hour.ago.iso8601
    assert_not_predicate bot, :restarting_within_interval?
  end

  test 'restarting_within_interval? returns false when pending equals effective' do
    bot = create(:dca_dual_asset, :stopped)
    bot.last_action_job_at = 1.hour.ago.iso8601
    assert_not_predicate bot, :restarting_within_interval?
  end

  test 'assets returns all three assets' do
    bot = create(:dca_dual_asset)
    assets = bot.assets
    assert_includes assets, bot.base0_asset
    assert_includes assets, bot.base1_asset
    assert_includes assets, bot.quote_asset
    assert_equal 3, assets.count
  end

  test 'base0_asset returns the first base asset' do
    bitcoin = create(:asset, :bitcoin)
    bot = create(:dca_dual_asset, base0_asset: bitcoin)
    assert_equal bitcoin, bot.base0_asset
  end

  test 'base1_asset returns the second base asset' do
    ethereum = create(:asset, :ethereum)
    bot = create(:dca_dual_asset, base1_asset: ethereum)
    assert_equal ethereum, bot.base1_asset
  end

  test 'quote_asset returns the quote asset' do
    usd = create(:asset, :usd)
    bot = create(:dca_dual_asset, quote_asset: usd)
    assert_equal usd, bot.quote_asset
  end

  test 'tickers returns both tickers' do
    bot = create(:dca_dual_asset)
    assert_equal 2, bot.tickers.count
  end

  test 'ticker0 returns the ticker for base0 asset' do
    bot = create(:dca_dual_asset)
    assert_equal bot.base0_asset, bot.ticker0.base_asset
    assert_equal bot.quote_asset, bot.ticker0.quote_asset
  end

  test 'ticker1 returns the ticker for base1 asset' do
    bot = create(:dca_dual_asset)
    assert_equal bot.base1_asset, bot.ticker1.base_asset
    assert_equal bot.quote_asset, bot.ticker1.quote_asset
  end

  test 'decimals returns decimal configuration for both base assets' do
    bot = create(:dca_dual_asset)
    decimals = bot.decimals
    assert decimals.key?(:base0)
    assert decimals.key?(:base1)
    assert decimals.key?(:quote)
    assert decimals.key?(:base0_price)
    assert decimals.key?(:base1_price)
  end

  test 'decimals returns empty hash when tickers are missing' do
    bot = create(:dca_dual_asset)
    bot.stubs(:ticker0).returns(nil)
    assert_equal({}, bot.decimals)
  end

  test 'available_exchanges returns exchanges that support both asset pairs' do
    bitcoin = create(:asset, :bitcoin)
    ethereum = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    binance = create(:binance_exchange)
    kraken = create(:kraken_exchange)

    create(:ticker, exchange: binance, base_asset: bitcoin, quote_asset: usd)
    create(:ticker, exchange: binance, base_asset: ethereum, quote_asset: usd)
    create(:ticker, exchange: kraken, base_asset: bitcoin, quote_asset: usd)
    create(:ticker, exchange: kraken, base_asset: ethereum, quote_asset: usd)

    bot = create(:dca_dual_asset, exchange: binance, base0_asset: bitcoin, base1_asset: ethereum, quote_asset: usd)
    available = bot.available_exchanges_for_current_settings
    assert_includes available, binance
    assert_includes available, kraken
  end

  test 'available_exchanges excludes exchanges missing one of the pairs' do
    bitcoin = create(:asset, :bitcoin)
    ethereum = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    binance = create(:binance_exchange)
    kraken = create(:kraken_exchange)

    create(:ticker, exchange: binance, base_asset: bitcoin, quote_asset: usd)
    create(:ticker, exchange: binance, base_asset: ethereum, quote_asset: usd)
    create(:ticker, exchange: kraken, base_asset: bitcoin, quote_asset: usd)
    create(:ticker, exchange: kraken, base_asset: ethereum, quote_asset: usd)

    bot = create(:dca_dual_asset, exchange: binance, base0_asset: bitcoin, base1_asset: ethereum, quote_asset: usd)

    coinbase = create(:coinbase_exchange)
    create(:ticker, exchange: coinbase, base_asset: bitcoin, quote_asset: usd)

    available = bot.available_exchanges_for_current_settings
    assert_not_includes available, coinbase
  end

  # == working? ==

  test 'working? returns true for scheduled status' do
    bot = build(:dca_dual_asset)
    bot.status = :scheduled
    assert_predicate bot, :working?
  end

  test 'working? returns true for executing status' do
    bot = build(:dca_dual_asset)
    bot.status = :executing
    assert_predicate bot, :working?
  end

  test 'working? returns true for retrying status' do
    bot = build(:dca_dual_asset)
    bot.status = :retrying
    assert_predicate bot, :working?
  end

  test 'working? returns true for waiting status' do
    bot = build(:dca_dual_asset)
    bot.status = :waiting
    assert_predicate bot, :working?
  end

  test 'working? returns false for created status' do
    bot = build(:dca_dual_asset)
    bot.status = :created
    assert_not_predicate bot, :working?
  end

  test 'working? returns false for stopped status' do
    bot = build(:dca_dual_asset)
    bot.status = :stopped
    assert_not_predicate bot, :working?
  end

  test 'working? returns false for deleted status' do
    bot = build(:dca_dual_asset)
    bot.status = :deleted
    assert_not_predicate bot, :working?
  end

  # == api_key_type ==

  test 'api_key_type returns trading' do
    bot = build(:dca_dual_asset)
    assert_equal :trading, bot.api_key_type
  end

  # == parse_params ==

  test 'parse_params extracts base0_asset_id' do
    bot = build(:dca_dual_asset)
    result = bot.parse_params(base0_asset_id: '123')
    assert_equal 123, result[:base0_asset_id]
  end

  test 'parse_params extracts base1_asset_id' do
    bot = build(:dca_dual_asset)
    result = bot.parse_params(base1_asset_id: '456')
    assert_equal 456, result[:base1_asset_id]
  end

  test 'parse_params extracts quote_asset_id' do
    bot = build(:dca_dual_asset)
    result = bot.parse_params(quote_asset_id: '789')
    assert_equal 789, result[:quote_asset_id]
  end

  test 'parse_params extracts quote_amount' do
    bot = build(:dca_dual_asset)
    result = bot.parse_params(quote_amount: '100.50')
    assert_equal 100.50, result[:quote_amount]
  end

  test 'parse_params extracts allocation0' do
    bot = build(:dca_dual_asset)
    result = bot.parse_params(allocation0: '0.7')
    assert_equal 0.7, result[:allocation0]
  end

  test 'parse_params extracts interval' do
    bot = build(:dca_dual_asset)
    result = bot.parse_params(interval: 'week')
    assert_equal 'week', result[:interval]
  end

  test 'parse_params ignores blank values' do
    bot = build(:dca_dual_asset)
    result = bot.parse_params(base0_asset_id: '', quote_amount: nil)
    assert_not result.key?(:base0_asset_id)
    assert_not result.key?(:quote_amount)
  end

  # == effective_quote_amount ==

  test 'effective_quote_amount returns quote_amount' do
    bot = build(:dca_dual_asset)
    bot.quote_amount = 150
    assert_equal 150, bot.effective_quote_amount
  end

  # == Concerns integration: Schedulable ==

  test 'provides interval_duration' do
    bot = create(:dca_dual_asset)
    assert_equal 1.day, bot.interval_duration
  end

  test 'provides effective_interval_duration' do
    bot = create(:dca_dual_asset)
    assert_equal 1.day, bot.effective_interval_duration
  end

  test 'next_interval_checkpoint_at calculates next checkpoint' do
    bot = create(:dca_dual_asset, :started)
    freeze_time do
      bot.update!(started_at: 2.hours.ago)
      next_checkpoint = bot.next_interval_checkpoint_at
      assert next_checkpoint > Time.current
      assert next_checkpoint < 1.day.from_now
    end
  end

  # == Concerns integration: Accountable ==

  test 'pending_quote_amount returns effective_quote_amount when no transactions' do
    bot = create(:dca_dual_asset, :started)
    assert_equal bot.effective_quote_amount, bot.pending_quote_amount
  end

  test 'pending_quote_amount subtracts invested amount' do
    bot = create(:dca_dual_asset, :started)
    create(:transaction, bot: bot, quote_amount_exec: 30, external_status: :closed, created_at: Time.current)
    bot.reload
    assert_equal 70, bot.pending_quote_amount
  end

  test 'set_missed_quote_amount caps at effective_quote_amount' do
    bot = create(:dca_dual_asset, :started)
    bot.set_missed_quote_amount
    # pending_quote_amount <= effective_quote_amount, so it's preserved as-is
    assert_equal bot.pending_quote_amount, bot.missed_quote_amount
  end

  # == STI ==

  test 'inherits from Bot' do
    assert_equal Bot, Bots::DcaDualAsset.superclass
  end

  test 'sets the correct type' do
    bot = create(:dca_dual_asset)
    assert_equal 'Bots::DcaDualAsset', bot.type
  end

  # == Factory ==

  test 'creates a valid bot with default settings' do
    bot = build(:dca_dual_asset)
    assert_predicate bot, :valid?
  end

  test 'creates associated exchange and assets' do
    bot = create(:dca_dual_asset)
    assert bot.exchange.present?
    assert bot.base0_asset.present?
    assert bot.base1_asset.present?
    assert bot.quote_asset.present?
    assert bot.ticker0.present?
    assert bot.ticker1.present?
  end

  test 'creates associated API key by default' do
    bot = create(:dca_dual_asset)
    api_key = ApiKey.find_by(user: bot.user, exchange: bot.exchange, key_type: :trading)
    assert api_key.present?
    assert_predicate api_key, :persisted?
  end

  test 'can skip API key creation' do
    bot = create(:dca_dual_asset, with_api_key: false)
    assert_predicate ApiKey.where(user: bot.user, exchange: bot.exchange), :empty?
  end

  test 'creates a started bot' do
    bot = create(:dca_dual_asset, :started)
    assert_predicate bot, :scheduled?
    assert bot.started_at.present?
  end

  test 'creates a stopped bot' do
    bot = create(:dca_dual_asset, :stopped)
    assert_predicate bot, :stopped?
    assert bot.stopped_at.present?
  end

  test 'creates an executing bot' do
    bot = create(:dca_dual_asset, :executing)
    assert_predicate bot, :executing?
  end

  test 'creates a waiting bot' do
    bot = create(:dca_dual_asset, :waiting)
    assert_predicate bot, :waiting?
  end

  test 'creates an hourly bot' do
    bot = create(:dca_dual_asset, :hourly)
    assert_equal 'hour', bot.interval
  end

  test 'creates a weekly bot' do
    bot = create(:dca_dual_asset, :weekly)
    assert_equal 'week', bot.interval
  end

  test 'creates a monthly bot' do
    bot = create(:dca_dual_asset, :monthly)
    assert_equal 'month', bot.interval
  end

  test 'creates a BTC-heavy bot' do
    bot = create(:dca_dual_asset, :btc_heavy)
    assert_equal 0.7, bot.allocation0
  end

  test 'creates an ETH-heavy bot' do
    bot = create(:dca_dual_asset, :eth_heavy)
    assert_equal 0.3, bot.allocation0
  end
end
