require 'test_helper'

class RuleLogTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @rule = Rules::Withdrawal.create!(
      user: @user, exchange: create(:binance_exchange), asset: create(:asset, :bitcoin),
      address: '1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa', max_fee_percentage: '1.0', status: :scheduled
    )
  end

  test 'status enum includes transient' do
    assert_includes RuleLog.statuses.keys, 'transient'
  end

  test 'log_transient creates a transient-status log' do
    @rule.send(:log_transient, 'temporary issue')
    log = @rule.rule_logs.last
    assert log.transient?
    assert_equal 'temporary issue', log.message
  end
end
