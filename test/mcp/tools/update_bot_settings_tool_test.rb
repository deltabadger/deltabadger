# frozen_string_literal: true

require 'test_helper'

class UpdateBotSettingsToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    @bot = create(:dca_single_asset, user: @user, status: :stopped)
    @user.set_mcp_tool_enabled('update_bot_settings', true)
    stub_mcp_client(@user)
  end

  teardown do
    ActionMCP::Current.reset
  end

  test 'updates quote_amount on a stopped bot' do
    response = UpdateBotSettingsTool.new(bot_id: @bot.id, quote_amount: 50.0).execute

    @bot.reload
    assert_equal 50.0, @bot.quote_amount.to_f
    assert_match(/updated/, response.contents.first.text)
  end

  test 'updates label' do
    UpdateBotSettingsTool.new(bot_id: @bot.id, label: 'My BTC Bot').execute

    @bot.reload
    assert_equal 'My BTC Bot', @bot.label
  end

  test 'rejects update on a running bot' do
    @bot.update!(status: :scheduled)

    response = UpdateBotSettingsTool.new(bot_id: @bot.id, quote_amount: 50.0).execute

    assert_match(/must be stopped/, response.contents.first.text)
  end

  test 'returns error for non-existent bot' do
    response = UpdateBotSettingsTool.new(bot_id: 999_999, quote_amount: 50.0).execute

    assert_match(/not found/, response.contents.first.text)
  end

  test 'returns error when tool is disabled' do
    @user.set_mcp_tool_enabled('update_bot_settings', false)

    response = UpdateBotSettingsTool.new(bot_id: @bot.id, quote_amount: 50.0).execute

    assert_match(/disabled/, response.contents.first.text)
  end

  test 'rejects when no settings provided' do
    response = UpdateBotSettingsTool.new(bot_id: @bot.id).execute

    assert_match(/No settings provided/, response.contents.first.text)
  end
  test 'an index bot is retuned through the same tool' do
    bot = create(:dca_index, user: @user, status: :stopped)

    response = UpdateBotSettingsTool.new(bot_id: bot.id, num_coins: 8, allocation_flattening: 0.5).execute

    assert_match(/settings updated/, response.contents.first.text)
    bot.reload
    assert_equal 8, bot.num_coins
    assert_equal 0.5, bot.allocation_flattening
  end

  test 'a basket is reweighted by symbol' do
    # The setup's single-asset bot already made BTC and USD; those traits pin external_id, so a
    # second create(:asset, :bitcoin) in the same test would collide.
    btc = Asset.find_by!(symbol: 'BTC')
    eth = create(:asset, :ethereum)
    bot = create(:dca_multi_asset, :stopped, user: @user, base_assets: [btc, eth],
                                             exchange: @bot.exchange, quote_asset: Asset.find_by!(symbol: 'USD'))

    response = UpdateBotSettingsTool.new(bot_id: bot.id, allocations: 'BTC:70,ETH:30').execute

    assert_match(/settings updated/, response.contents.first.text)
    assert_in_delta 0.7, bot.reload.allocation_for(btc.id), 0.0001
  end
end
