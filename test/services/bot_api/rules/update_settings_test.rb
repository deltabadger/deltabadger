# frozen_string_literal: true

require 'test_helper'

class BotApi::Rules::UpdateSettingsTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @asset = create(:asset, :bitcoin)
    @rule = Rules::Withdrawal.create!(
      user: @user, exchange: @exchange, asset: @asset,
      address: '0xabc123', status: :stopped,
      settings: { 'max_fee_percentage' => '5', 'threshold_type' => 'fee_percentage' }
    )
  end

  def update(**attrs) = BotApi::Rules::UpdateSettings.call(user: @user, rule_id: @rule.id, **attrs)

  test 'numbers and threshold types are checked at write time' do
    assert_equal 'invalid_number', update(withdrawal_percentage: 'abc').error_code
    assert_equal 'invalid_number', update(max_fee_percentage: '150').error_code
    assert_equal 'invalid_threshold_type', update(threshold_type: 'weekly').error_code
    # The inactive threshold's value is persisted too, and the web switch can activate it later.
    assert_equal 'invalid_number',
                 update(threshold_type: 'min_amount', min_amount: '0.25', max_fee_percentage: 'abc').error_code
    assert_equal '5', @rule.reload.max_fee_percentage, 'a refused update leaves the rule alone'
    assert update(threshold_type: 'min_amount', min_amount: '0.25').success?
    assert_equal 'min_amount', @rule.reload.threshold_type
  end

  # The resulting configuration is what has to hold, not just the fields that were sent.
  test 'switching to a threshold the rule has no value for is refused' do
    result = update(threshold_type: 'min_amount')

    assert_equal 'missing_required_parameter', result.error_code
    assert_equal 'fee_percentage', @rule.reload.threshold_type
  end

  test 'plain numeric updates still go through' do
    assert update(withdrawal_percentage: '75', max_fee_percentage: '2').success?
    @rule.reload
    assert_equal '75', @rule.withdrawal_percentage
    assert_equal '2', @rule.max_fee_percentage
  end

  test 'a deleted rule is out of reach of every rule tool' do
    @rule.delete

    assert_equal 'rule_not_found', update(withdrawal_percentage: '75').error_code
    assert_equal 'rule_not_found', BotApi::Rules::Start.call(user: @user, rule_id: @rule.id).error_code
    assert_equal 'rule_not_found', BotApi::Rules::Stop.call(user: @user, rule_id: @rule.id).error_code
  end
end
