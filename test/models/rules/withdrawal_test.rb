require 'test_helper'

class Rules::WithdrawalTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:binance_exchange)
    @asset = create(:asset)
    create(:exchange_asset, exchange: @exchange, asset: @asset)
    @user = create(:user)
    @ea = ExchangeAsset.find_by(exchange: @exchange, asset: @asset)
  end

  def build_rule(max_fee_percentage: '1.0')
    Rules::Withdrawal.new(
      user: @user,
      exchange: @exchange,
      asset: @asset,
      address: '1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa',
      max_fee_percentage: max_fee_percentage
    )
  end

  # withdrawal_fee_amount tests

  test 'withdrawal_fee_amount returns per-asset fee when available' do
    @ea.update!(withdrawal_fee: '0.0005', withdrawal_fee_updated_at: Time.current)
    rule = build_rule

    assert_equal BigDecimal('0.0005'), rule.withdrawal_fee_amount
  end

  test 'withdrawal_fee_amount returns zero when no per-asset fee exists' do
    rule = build_rule

    assert_equal BigDecimal('0'), rule.withdrawal_fee_amount
  end

  # withdrawal_fee_known? tests

  test 'withdrawal_fee_known? returns true when exchange_asset has withdrawal_fee' do
    @ea.update!(withdrawal_fee: '0.0005', withdrawal_fee_updated_at: Time.current)
    rule = build_rule

    assert rule.withdrawal_fee_known?
  end

  test 'withdrawal_fee_known? returns false when exchange_asset has no withdrawal_fee' do
    rule = build_rule

    assert_not rule.withdrawal_fee_known?
  end

  # minimum_withdrawal_amount with per-asset fee

  test 'minimum_withdrawal_amount uses per-asset fee' do
    @ea.update!(withdrawal_fee: '0.001', withdrawal_fee_updated_at: Time.current)
    rule = build_rule(max_fee_percentage: '1.0')

    # fee=0.001, pct=1% => 0.001 / 0.01 = 0.1
    assert_equal BigDecimal('0.1'), rule.minimum_withdrawal_amount
  end

  test 'minimum_withdrawal_amount returns nil when fee is zero' do
    @ea.update!(withdrawal_fee: '0.0', withdrawal_fee_updated_at: Time.current)
    rule = build_rule(max_fee_percentage: '1.0')

    assert_nil rule.minimum_withdrawal_amount
  end
end
