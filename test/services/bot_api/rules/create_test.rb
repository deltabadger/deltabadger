# frozen_string_literal: true

require 'test_helper'

class BotApi::Rules::CreateTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd)
    create(:api_key, user: @user, exchange: @exchange, key_type: :withdrawal, status: :correct)
    # The subclass overrides these, so a stub on Exchange would never run.
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:withdrawal_fee_fresh?).returns(true)
    @cold = 'bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh'
    Exchanges::Binance.any_instance.stubs(:list_withdrawal_addresses)
                      .returns([{ name: @cold, label: "#{@cold} - Cold" }])
  end

  def params(**overrides)
    { exchange_name: 'Binance', asset: 'BTC', address: @cold }.merge(overrides)
  end

  test 'creates a stopped rule with the wizard defaults when the fee is known' do
    Rules::Withdrawal.any_instance.stubs(:withdrawal_fee_known?).returns(true)
    result = BotApi::Rules::Create.call(user: @user, **params)

    assert result.success?, result.error_message
    assert_equal :created, result.status
    rule = @user.rules.find(result.data[:id])
    assert_equal 'Rules::Withdrawal', rule.type
    assert rule.created?
    assert_equal @cold, rule.address
    assert_equal '100', rule.withdrawal_percentage
    assert_equal 'fee_percentage', rule.threshold_type
    assert_equal '0.5', rule.max_fee_percentage
    assert_equal 'bc1qxy…0wlh', result.data[:address]
  end

  test 'an unknown fee defaults the threshold to a minimum amount, as the wizard does' do
    Rules::Withdrawal.any_instance.stubs(:withdrawal_fee_known?).returns(false)
    result = BotApi::Rules::Create.call(user: @user, **params)
    assert result.success?, result.error_message
    rule = @user.rules.find(result.data[:id])
    assert_equal 'min_amount', rule.threshold_type
    assert_equal '0.1', rule.min_amount
  end

  test 'stale fees are refreshed before the default is chosen' do
    Exchanges::Binance.any_instance.stubs(:withdrawal_fee_fresh?).returns(false)
    Exchanges::Binance.any_instance.expects(:fetch_withdrawal_fees!)
    Rules::Withdrawal.any_instance.stubs(:withdrawal_fee_known?).returns(true)
    assert BotApi::Rules::Create.call(user: @user, **params).success?
  end

  test 'an address the exchange does not list is refused without revealing the allow-list' do
    result = BotApi::Rules::Create.call(user: @user, **params(address: 'bc1qstranger'))

    assert_equal 'address_not_listed', result.error_code
    assert_nil result.data
    assert_no_match(/#{Regexp.escape(@cold)}/, result.error_message)
    assert_equal 0, @user.rules.count
  end

  test 'numbers are checked now, not when the rule is first started' do
    Rules::Withdrawal.any_instance.stubs(:withdrawal_fee_known?).returns(true)
    assert_equal 'invalid_number', BotApi::Rules::Create.call(user: @user, **params(withdrawal_percentage: 150)).error_code
    assert_equal 'invalid_number', BotApi::Rules::Create.call(user: @user, **params(withdrawal_percentage: '100.01')).error_code
    assert BotApi::Rules::Create.call(user: @user, **params(withdrawal_percentage: '100')).success?
    @user.rules.delete_all # destroy is the soft delete; the rows would still count
    assert_equal 'invalid_number', BotApi::Rules::Create.call(user: @user, **params(max_fee_percentage: 'abc')).error_code
    assert_equal 'invalid_number',
                 BotApi::Rules::Create.call(user: @user, **params(threshold_type: 'min_amount', min_amount: 0)).error_code
    assert_equal 'invalid_threshold_type', BotApi::Rules::Create.call(user: @user, **params(threshold_type: 'weekly')).error_code
    assert_equal 0, @user.rules.count
  end

  test 'a venue without withdrawals is refused' do
    Exchanges::Binance.any_instance.stubs(:supports_withdrawal?).returns(false)
    assert_equal 'withdrawal_unsupported', BotApi::Rules::Create.call(user: @user, **params).error_code
  end

  test 'the network defaults the way the wizard does, and an unlisted one is refused' do
    Rules::Withdrawal.any_instance.stubs(:withdrawal_fee_known?).returns(true)
    Rules::Withdrawal.any_instance.stubs(:available_chains)
                     .returns([{ 'name' => 'BTC', 'is_default' => false }, { 'name' => 'BSC', 'is_default' => true }])
    result = BotApi::Rules::Create.call(user: @user, **params)
    assert_equal 'BSC', @user.rules.find(result.data[:id]).network
    @user.rules.delete_all # destroy is the soft delete; the rows would still count

    assert_equal 'invalid_network', BotApi::Rules::Create.call(user: @user, **params(network: 'SOL')).error_code
    result = BotApi::Rules::Create.call(user: @user, **params(network: 'BTC'))
    assert_equal 'BTC', @user.rules.find(result.data[:id]).network
  end

  test 'a revived rule carries nothing over from its previous life' do
    Rules::Withdrawal.any_instance.stubs(:withdrawal_fee_known?).returns(true)
    first = @user.rules.find(BotApi::Rules::Create.call(user: @user, **params(address_tag: 'memo-1')).data[:id])
    first.update!(max_interval: '7')
    first.delete
    BotApi::Rules::Create.call(user: @user, **params)
    first.reload
    assert_nil first.address_tag
    assert_nil first.max_interval
  end

  test 'a concurrent create is a conflict, not a 500' do
    Rules::Withdrawal.any_instance.stubs(:withdrawal_fee_known?).returns(true)
    Rules::Withdrawal.any_instance.stubs(:save).raises(ActiveRecord::RecordNotUnique, 'UNIQUE constraint failed')
    result = BotApi::Rules::Create.call(user: @user, **params)
    assert_equal 'rule_exists', result.error_code
    assert_equal :conflict, result.status
  end

  test 'fails closed when the exchange cannot list addresses' do
    Exchanges::Binance.any_instance.stubs(:list_withdrawal_addresses).returns(nil)
    assert_equal 'address_not_listed', BotApi::Rules::Create.call(user: @user, **params).error_code
  end

  test 'needs a working withdrawal key' do
    @user.api_keys.destroy_all
    result = BotApi::Rules::Create.call(user: @user, **params)
    assert_equal 'withdrawal_key_missing', result.error_code
    assert_equal :permission_denied, result.status
  end

  test 'one rule per asset per exchange' do
    assert BotApi::Rules::Create.call(user: @user, **params).success?
    result = BotApi::Rules::Create.call(user: @user, **params)
    assert_equal 'rule_exists', result.error_code
    assert_equal :conflict, result.status
  end

  test 'a deleted rule for the same asset is revived rather than duplicated' do
    first = BotApi::Rules::Create.call(user: @user, **params).data[:id]
    @user.rules.find(first).delete
    second = BotApi::Rules::Create.call(user: @user, **params).data[:id]
    assert_equal first, second
    assert @user.rules.find(first).created?
  end

  test 'unknown asset on that exchange' do
    assert_equal 'asset_not_found', BotApi::Rules::Create.call(user: @user, **params(asset: 'XYZ')).error_code
  end

  test 'the list masks destinations and hides deleted rules' do
    Rules::Withdrawal.any_instance.stubs(:withdrawal_fee_known?).returns(true)
    id = BotApi::Rules::Create.call(user: @user, **params).data[:id]

    data = BotApi::Rules::List.call(user: @user).data
    assert_equal 1, data[:count]
    row = data[:rules].first
    assert_equal 'bc1qxy…0wlh', row[:address]
    assert_equal 'BTC', row[:asset]
    assert_equal 'Binance', row[:exchange]

    assert BotApi::Rules::Delete.call(user: @user, rule_id: id).success?
    assert @user.rules.find(id).deleted?
    assert_equal 0, BotApi::Rules::List.call(user: @user).data[:count]
    assert_equal 'rule_not_found', BotApi::Rules::Delete.call(user: @user, rule_id: id).error_code
  end

  test 'an active rule cannot be deleted' do
    Rules::Withdrawal.any_instance.stubs(:withdrawal_fee_known?).returns(true)
    id = BotApi::Rules::Create.call(user: @user, **params).data[:id]
    @user.rules.find(id).update!(status: :scheduled)

    result = BotApi::Rules::Delete.call(user: @user, rule_id: id)

    assert_equal 'rule_active', result.error_code
    assert_equal :conflict, result.status
  end

  test 'masking keeps a short destination unusable' do
    assert_equal '…', BotApi::Rules::List.mask('abcd')
    assert_equal 'ab…', BotApi::Rules::List.mask('abcdefghijkl')
    assert_equal 'abcdef…wxyz', BotApi::Rules::List.mask('abcdefghijklmnopqrstuvwxyz')
  end
end
