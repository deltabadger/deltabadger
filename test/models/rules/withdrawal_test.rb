require 'test_helper'

class Rules::WithdrawalTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:binance_exchange)
    @asset = create(:asset)
    create(:exchange_asset, exchange: @exchange, asset: @asset)
    @user = create(:user)
    @ea = ExchangeAsset.find_by(exchange: @exchange, asset: @asset)
  end

  def build_rule(max_fee_percentage: '1.0', threshold_type: 'fee_percentage', min_amount: nil)
    Rules::Withdrawal.new(
      user: @user,
      exchange: @exchange,
      asset: @asset,
      address: '1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa',
      threshold_type: threshold_type,
      max_fee_percentage: max_fee_percentage,
      min_amount: min_amount
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

  # minimum_withdrawal_amount with fee_percentage mode

  test 'minimum_withdrawal_amount uses per-asset fee in fee_percentage mode' do
    @ea.update!(withdrawal_fee: '0.001', withdrawal_fee_updated_at: Time.current)
    rule = build_rule(max_fee_percentage: '1.0')

    # fee=0.001, pct=1% => 0.001 / 0.01 = 0.1
    assert_equal BigDecimal('0.1'), rule.minimum_withdrawal_amount
  end

  test 'minimum_withdrawal_amount returns nil when fee is zero in fee_percentage mode' do
    @ea.update!(withdrawal_fee: '0.0', withdrawal_fee_updated_at: Time.current)
    rule = build_rule(max_fee_percentage: '1.0')

    assert_nil rule.minimum_withdrawal_amount
  end

  # minimum_withdrawal_amount with min_amount mode

  test 'minimum_withdrawal_amount returns fixed amount in min_amount mode' do
    rule = build_rule(threshold_type: 'min_amount', min_amount: '0.05')

    assert_equal BigDecimal('0.05'), rule.minimum_withdrawal_amount
  end

  test 'minimum_withdrawal_amount returns nil when min_amount is blank' do
    rule = build_rule(threshold_type: 'min_amount', min_amount: nil)

    assert_nil rule.minimum_withdrawal_amount
  end

  test 'minimum_withdrawal_amount ignores fee in min_amount mode' do
    @ea.update!(withdrawal_fee: '0.001', withdrawal_fee_updated_at: Time.current)
    rule = build_rule(threshold_type: 'min_amount', min_amount: '0.5')

    assert_equal BigDecimal('0.5'), rule.minimum_withdrawal_amount
  end

  # Chain-specific fee tests

  test 'withdrawal_fee_amount uses selected chain fee when network is set' do
    @ea.update!(
      withdrawal_fee: '0.0005',
      withdrawal_fee_updated_at: Time.current,
      withdrawal_chains: [
        { 'name' => 'BTC', 'fee' => '0.0005', 'is_default' => true },
        { 'name' => 'BEP20', 'fee' => '0.00001', 'is_default' => false }
      ]
    )
    rule = build_rule
    rule.network = 'BEP20'

    assert_equal BigDecimal('0.00001'), rule.withdrawal_fee_amount
  end

  test 'withdrawal_fee_amount falls back to default when network not in chains' do
    @ea.update!(
      withdrawal_fee: '0.0005',
      withdrawal_fee_updated_at: Time.current,
      withdrawal_chains: [
        { 'name' => 'BTC', 'fee' => '0.0005', 'is_default' => true }
      ]
    )
    rule = build_rule
    rule.network = 'NONEXISTENT'

    assert_equal BigDecimal('0.0005'), rule.withdrawal_fee_amount
  end

  test 'withdrawal_fee_amount falls back to default when network is blank' do
    @ea.update!(withdrawal_fee: '0.0005', withdrawal_fee_updated_at: Time.current)
    rule = build_rule

    assert_equal BigDecimal('0.0005'), rule.withdrawal_fee_amount
  end

  test 'minimum_withdrawal_amount uses selected chain fee' do
    @ea.update!(
      withdrawal_fee: '0.0005',
      withdrawal_fee_updated_at: Time.current,
      withdrawal_chains: [
        { 'name' => 'BTC', 'fee' => '0.0005', 'is_default' => true },
        { 'name' => 'BEP20', 'fee' => '0.00001', 'is_default' => false }
      ]
    )
    rule = build_rule(max_fee_percentage: '1.0')
    rule.network = 'BEP20'

    # fee=0.00001, pct=1% => 0.00001 / 0.01 = 0.001
    assert_equal BigDecimal('0.001'), rule.minimum_withdrawal_amount
  end

  # parse_params tests

  test 'parse_params updates max_fee_percentage' do
    rule = build_rule
    rule.parse_params(max_fee_percentage: '2.5')

    assert_equal '2.5', rule.max_fee_percentage
  end

  test 'parse_params updates network' do
    rule = build_rule
    rule.parse_params(network: 'BEP20')

    assert_equal 'BEP20', rule.network
  end

  test 'parse_params updates address_tag' do
    rule = build_rule
    rule.parse_params(address_tag: 'memo456')

    assert_equal 'memo456', rule.address_tag
  end

  test 'parse_params clears network with nil' do
    rule = build_rule
    rule.network = 'BTC'
    rule.parse_params(network: nil)

    assert_nil rule.network
  end

  test 'parse_params updates threshold_type' do
    rule = build_rule
    rule.parse_params(threshold_type: 'min_amount')

    assert_equal 'min_amount', rule.threshold_type
  end

  test 'parse_params updates min_amount' do
    rule = build_rule(threshold_type: 'min_amount')
    rule.parse_params(min_amount: '0.25')

    assert_equal '0.25', rule.min_amount
  end

  # Validation tests

  test 'validates max_fee_percentage required in fee_percentage mode' do
    rule = build_rule(max_fee_percentage: nil, threshold_type: 'fee_percentage')

    assert_not rule.valid?
    assert rule.errors[:max_fee_percentage].any?
  end

  test 'does not validate max_fee_percentage in min_amount mode' do
    rule = build_rule(max_fee_percentage: nil, threshold_type: 'min_amount', min_amount: '0.1')

    assert rule.valid?
  end

  test 'validates min_amount required in min_amount mode' do
    rule = build_rule(threshold_type: 'min_amount', min_amount: nil)

    assert_not rule.valid?
    assert rule.errors[:min_amount].any?
  end

  test 'validates min_amount must be positive' do
    rule = build_rule(threshold_type: 'min_amount', min_amount: '0')

    assert_not rule.valid?
    assert rule.errors[:min_amount].any?
  end

  test 'does not validate min_amount in fee_percentage mode' do
    rule = build_rule(threshold_type: 'fee_percentage', min_amount: nil)

    assert rule.valid?
  end
end
