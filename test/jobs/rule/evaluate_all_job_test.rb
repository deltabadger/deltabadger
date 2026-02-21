require 'test_helper'

class Rule::EvaluateAllJobTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:binance_exchange)
    @asset = create(:asset, :bitcoin)
    @quote_asset = create(:asset, :usd)
    create(:ticker, exchange: @exchange, base_asset: @asset, quote_asset: @quote_asset)
    @user = create(:user)
  end

  test 'enqueues ExecuteJob for each scheduled rule' do
    rule1 = Rules::Withdrawal.create!(
      user: @user, exchange: @exchange, asset: @asset,
      address: 'addr1', max_fee_percentage: '1.0', status: :scheduled
    )
    rule2 = Rules::Withdrawal.create!(
      user: create(:user), exchange: @exchange, asset: @asset,
      address: 'addr2', max_fee_percentage: '2.0', status: :scheduled
    )

    Rule::ExecuteJob.expects(:perform_later).with(rule1).once
    Rule::ExecuteJob.expects(:perform_later).with(rule2).once

    Rule::EvaluateAllJob.perform_now
  end

  test 'does not enqueue for stopped rules' do
    Rules::Withdrawal.create!(
      user: @user, exchange: @exchange, asset: @asset,
      address: 'addr1', max_fee_percentage: '1.0', status: :stopped
    )

    Rule::ExecuteJob.expects(:perform_later).never

    Rule::EvaluateAllJob.perform_now
  end

  test 'does not enqueue for created rules' do
    Rules::Withdrawal.create!(
      user: @user, exchange: @exchange, asset: @asset,
      address: 'addr1', max_fee_percentage: '1.0', status: :created
    )

    Rule::ExecuteJob.expects(:perform_later).never

    Rule::EvaluateAllJob.perform_now
  end

  test 'does not enqueue for deleted rules' do
    Rules::Withdrawal.create!(
      user: @user, exchange: @exchange, asset: @asset,
      address: 'addr1', max_fee_percentage: '1.0', status: :deleted
    )

    Rule::ExecuteJob.expects(:perform_later).never

    Rule::EvaluateAllJob.perform_now
  end
end
