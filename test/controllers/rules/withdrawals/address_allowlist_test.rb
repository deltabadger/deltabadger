# frozen_string_literal: true

require 'test_helper'

# A withdrawal rule may only pay out to an address the exchange itself already holds on
# its withdrawal allowlist. That check ran while preparing the confirmation screen, so it
# bounded what the screen offered rather than what the app accepted: a request that skipped
# the screen skipped the check with it. These pin the two places a raw address can enter.
class Rules::Withdrawals::AddressAllowlistTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  ALLOWED   = 'allowlisted-address'
  SECOND    = 'second-allowlisted-address'
  ARBITRARY = 'attacker-chosen-address'

  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    @exchange = create(:binance_exchange)
    @asset = create(:asset, :bitcoin)
    create(:api_key, user: @user, exchange: @exchange, key_type: :withdrawal, status: :correct)

    Exchanges::Binance.any_instance.stubs(:list_withdrawal_addresses)
                      .returns([{ name: ALLOWED, network: 'BTC' }])

    # The address step reads the asset and exchange the earlier steps put in the session, so
    # they have to be walked rather than assumed — otherwise every request below is refused
    # for a missing asset and the assertions pass without exercising the allowlist at all.
    post rules_withdrawals_pick_exchange_path, params: { bots_dca_single_asset: { exchange_id: @exchange.id } }
    post rules_withdrawals_pick_asset_path, params: { bots_dca_single_asset: { asset_id: @asset.id } }

    assert_equal @asset.id.to_s, session.dig('withdrawal_rule_config', 'asset_id').to_s
    assert_equal @exchange.id.to_s, session.dig('withdrawal_rule_config', 'exchange_id').to_s
  end

  # The confirmation screen is where the destination is chosen, from a select built out of
  # the exchange's own allowlist — so this step has to keep accepting an address, and has to
  # check the one it is given.
  test 'the confirmation step refuses an address the exchange does not hold' do
    post rules_withdrawals_confirm_settings_path, params: {
      address: ARBITRARY, withdrawal_percentage: 100, threshold_type: 'min_amount', min_amount: 1
    }

    assert_redirected_to new_rules_withdrawals_add_address_path
    assert_nil @user.rules.find_by(type: 'Rules::Withdrawal'),
               'a rule must not be created for an address the exchange never listed'
  end

  # The half that a params-only fix would have broken: with several allowlisted addresses the
  # user picks one here, and that choice has to reach the rule rather than the first default.
  test 'the confirmation step honours the allowlisted address the user picked' do
    Exchanges::Binance.any_instance.unstub(:list_withdrawal_addresses)
    Exchanges::Binance.any_instance.stubs(:list_withdrawal_addresses)
                      .returns([{ name: ALLOWED, network: 'BTC' }, { name: SECOND, network: 'BTC' }])

    post rules_withdrawals_confirm_settings_path, params: {
      address: SECOND, withdrawal_percentage: 100, threshold_type: 'min_amount', min_amount: 1
    }

    rule = @user.rules.find_by(type: 'Rules::Withdrawal')

    assert_equal SECOND, rule&.address, 'the rule must pay out to the address that was chosen'
  end

  # The wizard step that does take an address has to be the one that checks it, because
  # it is the only point where the value arrives from outside.
  test 'the address step refuses an address the exchange does not hold' do
    post rules_withdrawals_add_address_path, params: { address: ARBITRARY }

    assert_redirected_to new_rules_withdrawals_add_address_path
    assert_nil session.dig('withdrawal_rule_config', 'address'),
               'an unlisted address must not reach the session'
  end

  test 'the address step accepts an address the exchange holds' do
    post rules_withdrawals_add_address_path, params: { address: ALLOWED }

    assert_redirected_to new_rules_withdrawals_confirm_settings_path
    assert_equal ALLOWED, session.dig('withdrawal_rule_config', 'address')
  end

  # Failing open would turn an exchange outage into a way through.
  test 'an address is refused when the allowlist cannot be read' do
    Exchanges::Binance.any_instance.unstub(:list_withdrawal_addresses)
    Exchanges::Binance.any_instance.stubs(:list_withdrawal_addresses).returns(nil)

    post rules_withdrawals_add_address_path, params: { address: ALLOWED }

    assert_nil session.dig('withdrawal_rule_config', 'address')
  end
end
