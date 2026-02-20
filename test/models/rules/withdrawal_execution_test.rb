require 'test_helper'

class Rules::WithdrawalExecutionTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:binance_exchange)
    @asset = create(:asset, :bitcoin)
    @quote_asset = create(:asset, :usd)
    create(:ticker, exchange: @exchange, base_asset: @asset, quote_asset: @quote_asset)
    @user = create(:user)
    @ea = ExchangeAsset.find_by(exchange: @exchange, asset: @asset)
    @ea.update!(withdrawal_fee: '0.0005', withdrawal_fee_updated_at: Time.current)

    @api_key = create(:api_key, user: @user, exchange: @exchange, key_type: :withdrawal)

    @rule = Rules::Withdrawal.create!(
      user: @user,
      exchange: @exchange,
      asset: @asset,
      address: '1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa',
      max_fee_percentage: '1.0',
      status: :scheduled
    )
  end

  # Automation interface tests

  test 'api_key_type returns :withdrawal' do
    assert_equal :withdrawal, @rule.api_key_type
  end

  test 'start sets status to scheduled' do
    @rule.update!(status: :stopped)
    @rule.start
    assert @rule.scheduled?
  end

  test 'stop sets status to stopped' do
    @rule.stop
    assert @rule.stopped?
  end

  test 'delete sets status to deleted' do
    @rule.delete
    assert @rule.deleted?
  end

  test 'scheduled? returns true for active rules' do
    assert @rule.scheduled?
  end

  test 'stopped? returns true after stop' do
    @rule.stop
    assert @rule.stopped?
  end

  # Execute with sufficient balance

  test 'execute withdraws when balance exceeds minimum' do
    # min_amount = 0.0005 / 0.01 = 0.05
    # balance = 1.0, amount = 1.0 - 0.0005 = 0.9995
    stub_exchange_balance(@exchange, asset_id: @asset.id, free: 1.0, locked: 0)
    @exchange.stubs(:withdrawal_fee_fresh?).returns(true)
    @exchange.stubs(:withdraw).returns(Result::Success.new({ withdrawal_id: 'test-123' }))

    result = @rule.execute

    assert result.success?
    assert_equal 1, @rule.rule_logs.count
    assert @rule.rule_logs.last.success?
    assert_includes @rule.rule_logs.last.message, 'Withdrew'
    assert_equal 'test-123', @rule.rule_logs.last.details['withdrawal_id']
  end

  # Execute with insufficient balance

  test 'execute skips when balance below minimum' do
    # min_amount = 0.0005 / 0.01 = 0.05
    # balance = 0.01 < 0.05
    stub_exchange_balance(@exchange, asset_id: @asset.id, free: 0.01, locked: 0)

    result = @rule.execute

    assert result.success?
    assert result.data[:skipped]
    assert_equal 1, @rule.rule_logs.count
    assert @rule.rule_logs.last.pending?
    assert_includes @rule.rule_logs.last.message, 'below minimum'
  end

  # Execute when balance doesn't cover fee

  test 'execute skips when balance equals fee' do
    # With fee=0.0005 and max_fee_percentage=1%, min_amount = 0.05
    # Balance 0.0005 < 0.05, so skipped at the minimum check
    stub_exchange_balance(@exchange, asset_id: @asset.id, free: 0.0005, locked: 0)
    @exchange.stubs(:withdrawal_fee_fresh?).returns(true)

    result = @rule.execute

    assert result.success?
    assert result.data[:skipped]
    assert_equal 1, @rule.rule_logs.count
    assert_includes @rule.rule_logs.last.message, 'below minimum'
  end

  test 'execute skips when balance barely covers fee but amount would be zero' do
    # Set fee high enough that balance - fee <= 0
    @ea.update!(withdrawal_fee: '1.0', withdrawal_fee_updated_at: Time.current)
    # Override max_fee_percentage to 100% so minimum_withdrawal_amount = fee/1 = 1.0
    @rule.update!(max_fee_percentage: '100')

    stub_exchange_balance(@exchange, asset_id: @asset.id, free: 1.0, locked: 0)
    @exchange.stubs(:withdrawal_fee_fresh?).returns(true)

    result = @rule.execute

    assert result.success?
    assert result.data[:skipped]
    assert_equal 1, @rule.rule_logs.count
    assert_includes @rule.rule_logs.last.message, 'does not cover fee'
  end

  # Execute when get_balance fails

  test 'execute logs failure when get_balance fails' do
    @exchange.stubs(:get_balance).returns(Result::Failure.new('API error'))

    result = @rule.execute

    assert result.failure?
    assert_equal 1, @rule.rule_logs.count
    assert @rule.rule_logs.last.failed?
    assert_includes @rule.rule_logs.last.message, 'Failed to fetch balance'
  end

  # Execute when withdraw fails

  test 'execute logs failure when withdraw fails' do
    stub_exchange_balance(@exchange, asset_id: @asset.id, free: 1.0, locked: 0)
    @exchange.stubs(:withdrawal_fee_fresh?).returns(true)
    @exchange.stubs(:withdraw).returns(Result::Failure.new('Withdrawal disabled'))

    result = @rule.execute

    assert result.failure?
    assert_equal 1, @rule.rule_logs.count
    assert @rule.rule_logs.last.failed?
    assert_includes @rule.rule_logs.last.message, 'Withdrawal failed'
  end

  # Execute refreshes stale fee

  test 'execute refreshes fee when stale' do
    stub_exchange_balance(@exchange, asset_id: @asset.id, free: 1.0, locked: 0)
    @exchange.stubs(:withdrawal_fee_fresh?).returns(false)
    @exchange.stubs(:fetch_withdrawal_fees!).returns(Result::Success.new({}))
    @exchange.stubs(:withdraw).returns(Result::Success.new({ withdrawal_id: 'fresh-fee-123' }))

    result = @rule.execute

    assert result.success?
    assert_equal 1, @rule.rule_logs.count
    assert @rule.rule_logs.last.success?
  end

  # Execute when fee refresh fails

  test 'execute logs failure when fee refresh fails' do
    stub_exchange_balance(@exchange, asset_id: @asset.id, free: 1.0, locked: 0)
    @exchange.stubs(:withdrawal_fee_fresh?).returns(false)
    @exchange.stubs(:fetch_withdrawal_fees!).returns(Result::Failure.new('Fee API error'))

    result = @rule.execute

    assert result.failure?
    assert_equal 1, @rule.rule_logs.count
    assert @rule.rule_logs.last.failed?
    assert_includes @rule.rule_logs.last.message, 'Failed to refresh withdrawal fees'
  end

  # Dry mode test

  test 'withdraw returns mock result in dry mode' do
    stub_exchange_balance(@exchange, asset_id: @asset.id, free: 1.0, locked: 0)
    @exchange.stubs(:withdrawal_fee_fresh?).returns(true)
    # In dry mode, Exchange::Dryable will intercept the withdraw call
    # and return a mock result

    result = @rule.execute

    assert result.success?
    assert_equal 1, @rule.rule_logs.count
    assert @rule.rule_logs.last.success?
    assert @rule.rule_logs.last.details['withdrawal_id'].start_with?('dry-withdrawal-')
  end

  # Network and address_tag passthrough

  test 'execute passes network and address_tag to withdraw' do
    @rule.update!(network: 'BEP20', address_tag: 'memo123')
    stub_exchange_balance(@exchange, asset_id: @asset.id, free: 1.0, locked: 0)
    @exchange.stubs(:withdrawal_fee_fresh?).returns(true)

    @exchange.expects(:withdraw).with(
      asset: @asset,
      amount: anything,
      address: @rule.address,
      network: 'BEP20',
      address_tag: 'memo123'
    ).returns(Result::Success.new({ withdrawal_id: 'net-123' }))

    result = @rule.execute
    assert result.success?
    assert_equal 'BEP20', @rule.rule_logs.last.details['network']
    assert_equal 'memo123', @rule.rule_logs.last.details['address_tag']
  end

  test 'execute passes nil network when not set' do
    stub_exchange_balance(@exchange, asset_id: @asset.id, free: 1.0, locked: 0)
    @exchange.stubs(:withdrawal_fee_fresh?).returns(true)

    @exchange.expects(:withdraw).with(
      asset: @asset,
      amount: anything,
      address: @rule.address,
      network: nil,
      address_tag: nil
    ).returns(Result::Success.new({ withdrawal_id: 'no-net-123' }))

    result = @rule.execute
    assert result.success?
  end

  # Available chains helper

  test 'available_chains returns chains from exchange_asset' do
    chains = [{ 'name' => 'BTC', 'fee' => '0.0005', 'is_default' => true },
              { 'name' => 'BEP20', 'fee' => '0.00001', 'is_default' => false }]
    @ea.update!(withdrawal_chains: chains)

    assert_equal 2, @rule.available_chains.size
    assert_equal 'BTC', @rule.available_chains.first['name']
  end

  test 'available_chains returns empty array when no chains' do
    assert_equal [], @rule.available_chains
  end

  # Execute when fee is unknown (zero)

  test 'execute refreshes fee when fee is zero and minimum_withdrawal_amount is nil' do
    @ea.update!(withdrawal_fee: nil, withdrawal_fee_updated_at: nil)

    stub_exchange_balance(@exchange, asset_id: @asset.id, free: 1.0, locked: 0)
    @exchange.stubs(:fetch_withdrawal_fees!).returns(Result::Success.new({}))
    @exchange.stubs(:withdrawal_fee_fresh?).returns(true)
    @exchange.stubs(:withdraw).returns(Result::Success.new({ withdrawal_id: 'no-fee-123' }))

    result = @rule.execute

    assert result.success?
  end
end
