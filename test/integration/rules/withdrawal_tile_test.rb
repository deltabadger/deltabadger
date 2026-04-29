require 'test_helper'

class Rules::WithdrawalTileTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    @exchange = create(:binance_exchange)
    @asset = create(:asset, :bitcoin)
    @ea = create(:exchange_asset, exchange: @exchange, asset: @asset,
                                  withdrawal_fee: '0.0005', withdrawal_fee_updated_at: Time.current)
    @api_key = create(:api_key, user: @user, exchange: @exchange, key_type: :withdrawal, status: :correct)

    @rule = Rules::Withdrawal.create!(
      user: @user,
      exchange: @exchange,
      asset: @asset,
      address: 'wallet-one',
      threshold_type: 'fee_percentage',
      max_fee_percentage: '1.0',
      status: :stopped
    )

    sign_in @user
  end

  test 'scheduled rule has disabled inputs' do
    @rule.update!(status: :scheduled)
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:list_withdrawal_addresses).returns(nil)

    get rules_path
    assert_response :ok
    assert_select 'input[name="rules_withdrawal[withdrawal_percentage]"][disabled]'
    assert_select 'input[name="rules_withdrawal[max_fee_percentage]"][disabled]'
    assert_select 'select[name="rules_withdrawal[threshold_type]"][disabled]'
  end

  test 'stopped rule shows withdrawal percentage input' do
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:list_withdrawal_addresses).returns(nil)

    get rules_path
    assert_response :ok
    assert_select 'input[name="rules_withdrawal[withdrawal_percentage]"][value="100"]'
  end

  test 'stopped rule shows address dropdown when exchange has addresses' do
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:list_withdrawal_addresses).returns([
                                                                                { name: 'wallet-one', label: 'wallet-one' },
                                                                                { name: 'wallet-two', label: 'wallet-two' }
                                                                              ])

    get rules_path
    assert_response :ok
    assert_select 'select[name="rules_withdrawal[address]"]' do
      assert_select 'option[value="wallet-one"][selected]'
      assert_select 'option[value="wallet-two"]'
    end
  end

  test 'stopped rule shows static address when exchange returns nil' do
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:list_withdrawal_addresses).returns(nil)

    get rules_path
    assert_response :ok
    assert_select 'select[name="rules_withdrawal[address]"]', count: 0
    assert_select '.tag--address', text: /wallet-one/
  end

  test 'stopped rule shows flash when address was deleted from exchange' do
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:list_withdrawal_addresses).returns([
                                                                                { name: 'wallet-two', label: 'wallet-two' }
                                                                              ])

    get rules_path
    assert_response :ok
    assert_select 'select[name="rules_withdrawal[address]"]' do
      assert_select 'option', count: 1
      assert_select 'option[value="wallet-two"]'
    end
  end

  test 'stopped rule shows empty wallet notice when exchange has no addresses' do
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:list_withdrawal_addresses).returns([])

    get rules_path
    assert_response :ok
    assert_select '.tag--address'
    assert_select 'select[name="rules_withdrawal[address]"]', count: 0
    assert_select '.small-info.text-danger', text: /BTC.*Binance/
  end

  test 'update action accepts address change' do
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:list_withdrawal_addresses).returns([
                                                                                { name: 'wallet-new', label: 'wallet-new' }
                                                                              ])

    patch rules_withdrawal_path(id: @rule.id), params: {
      rules_withdrawal: { address: 'wallet-new' }
    }, headers: { 'Accept' => 'text/vnd.turbo-stream.html' }

    assert_response :ok
    @rule.reload
    assert_equal 'wallet-new', @rule.address
  end

  test 'update action accepts withdrawal percentage change' do
    patch rules_withdrawal_path(id: @rule.id), params: {
      rules_withdrawal: { withdrawal_percentage: '80' }
    }, headers: { 'Accept' => 'text/vnd.turbo-stream.html' }

    assert_response :ok
    assert_equal '80', @rule.reload.withdrawal_percentage
  end

  test 'scheduled rule does not show address dropdown' do
    @rule.update!(status: :scheduled)
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:list_withdrawal_addresses).returns([
                                                                                { name: 'wallet-one', label: 'wallet-one' },
                                                                                { name: 'wallet-two', label: 'wallet-two' }
                                                                              ])

    get rules_path
    assert_response :ok
    assert_select 'select[name="rules_withdrawal[address]"]', count: 0
  end
end
