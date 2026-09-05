# frozen_string_literal: true

require 'test_helper'

# list_rules / create_rule / delete_rule share one fixture: a Binance withdrawal key, a BTC pair
# and one allow-listed address.
class RuleLifecycleToolsTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @exchange = create(:binance_exchange)
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @usd)
    create(:api_key, user: @user, exchange: @exchange, key_type: :withdrawal, status: :correct)
    @cold = 'bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh'
    # The subclass overrides these, so a stub on Exchange would never run.
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:withdrawal_fee_fresh?).returns(true)
    Exchanges::Binance.any_instance.stubs(:list_withdrawal_addresses)
                      .returns([{ name: @cold, label: "#{@cold} - Cold" }])
    Rules::Withdrawal.any_instance.stubs(:withdrawal_fee_known?).returns(true)
    %w[list_rules create_rule delete_rule].each { |tool| @user.set_mcp_tool_enabled(tool, true) }
    stub_mcp_client(@user)
  end

  teardown { ActionMCP::Current.reset }

  test 'create_rule creates a stopped rule and list_rules shows it with a masked address' do
    text = CreateRuleTool.new(exchange_name: 'Binance', asset: 'BTC', address: @cold).execute.contents.first.text
    assert_match(/Rule #\d+ created/, text)

    listed = ListRulesTool.new.execute.contents.first.text
    assert_match(/Rules \(1\)/, listed)
    assert_match(/bc1qxy…0wlh/, listed)
    assert_no_match(/#{Regexp.escape(@cold)}/, listed)
  end

  test 'create_rule refuses an address the exchange does not list, and says nothing more' do
    text = CreateRuleTool.new(exchange_name: 'Binance', asset: 'BTC', address: 'bc1qstranger').execute.contents.first.text

    assert_match(/not in Binance's withdrawal allow-list/, text)
    assert_no_match(/#{Regexp.escape(@cold)}/, text)
    assert_equal 0, @user.rules.count
  end

  test 'delete_rule deletes a stopped rule' do
    id = BotApi::Rules::Create.call(user: @user, exchange_name: 'Binance', asset: 'BTC', address: @cold).data[:id]

    text = DeleteRuleTool.new(rule_id: id).execute.contents.first.text

    assert_match(/deleted/, text)
    assert @user.rules.find(id).deleted?
  end

  test 'list_rules says so when there are none' do
    assert_match(/No rules found/, ListRulesTool.new.execute.contents.first.text)
  end
end
