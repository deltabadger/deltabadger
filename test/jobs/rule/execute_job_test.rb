require 'test_helper'

class Rule::ExecuteJobTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:binance_exchange)
    @asset = create(:asset, :bitcoin)
    @quote_asset = create(:asset, :usd)
    create(:ticker, exchange: @exchange, base_asset: @asset, quote_asset: @quote_asset)
    @user = create(:user)
    create(:api_key, user: @user, exchange: @exchange, key_type: :withdrawal)

    @ea = ExchangeAsset.find_by(exchange: @exchange, asset: @asset)
    @ea.update!(withdrawal_fee: '0.0005', withdrawal_fee_updated_at: Time.current)

    @rule = Rules::Withdrawal.create!(
      user: @user, exchange: @exchange, asset: @asset,
      address: '1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa',
      max_fee_percentage: '1.0', status: :scheduled
    )
  end

  test 'calls execute on a scheduled rule' do
    stub_exchange_balance(@exchange, asset_id: @asset.id, free: 1.0, locked: 0)
    @exchange.stubs(:withdrawal_fee_fresh?).returns(true)
    @exchange.stubs(:withdraw).returns(Result::Success.new({ withdrawal_id: 'job-test-123' }))

    Rule::ExecuteJob.perform_now(@rule)

    assert_equal 1, @rule.rule_logs.count
    assert @rule.rule_logs.last.success?
  end

  test 'skips non-scheduled rules' do
    @rule.update!(status: :stopped)

    Rule::ExecuteJob.perform_now(@rule)

    assert_equal 0, @rule.rule_logs.count
  end

  test 'catches and logs unexpected exceptions' do
    @rule.stubs(:execute).raises(StandardError, 'Something broke')

    Rule::ExecuteJob.perform_now(@rule)

    assert_equal 1, @rule.rule_logs.count
    assert @rule.rule_logs.last.failed?
    assert_includes @rule.rule_logs.last.message, 'Something broke'
  end
end
