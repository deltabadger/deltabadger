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

  test 'the schema offers every settings-page field with the page choices' do
    schema = UpdateBotSettingsTool.to_h[:inputSchema]
    assert_equal ['bot_id', *BotApi::Bots::SettingsFields::NAMES], schema[:properties].keys
    assert_equal Automation::Schedulable::INTERVALS.keys, schema[:properties]['interval'][:enum]
    assert_equal Bot::Startable::MODES, schema[:properties]['start_time_mode'][:enum]
    assert_equal %w[restrict start flip], schema[:properties]['price_limit_mode'][:enum]
    assert_equal %w[start flip], schema[:properties]['sell_price_drop_limit_mode'][:enum]
    assert_equal %w[twenty_four_hours seven_days], schema[:properties]['sell_price_drop_limit_time_window_condition'][:enum]
    %w[sell_amount sell_quote_amount rebalance_threshold].each { |name| assert_equal %w[number null], schema[:properties][name][:type] }
    assert_equal %w[string null], schema[:properties]['start_at'][:type]
    assert_equal 'number', schema[:properties]['quote_amount'][:type], 'only fields the page can blank take null'
  end

  test 'a starting time is set through the tool' do
    response = UpdateBotSettingsTool.from_wire('bot_id' => @bot.id, 'start_time_enabled' => true, 'start_time_mode' => 'hour',
                                               'start_time_of_day' => '09:30').execute

    assert_match(/settings updated: start_time_enabled, start_time_mode, start_time_of_day/, response.contents.first.text)
    @bot.reload
    assert_equal [true, 'hour', '09:30'], [@bot.start_time_enabled, @bot.start_time_mode, @bot.start_time_of_day]
  end

  test 'an omitted field stays, null clears, a value sets' do
    @bot.settings['start_at'] = '2026-12-01T10:00:00Z'
    @bot.set_missed_quote_amount
    @bot.save!

    UpdateBotSettingsTool.from_wire('bot_id' => @bot.id, 'label' => 'Other').execute
    assert_equal '2026-12-01T10:00:00Z', @bot.reload.start_at

    response = UpdateBotSettingsTool.from_wire('bot_id' => @bot.id, 'start_at' => nil).execute
    assert_match(/settings updated: start_at/, response.contents.first.text)
    assert @bot.reload.settings.key?('start_at')
    assert_nil @bot.start_at

    UpdateBotSettingsTool.from_wire('bot_id' => @bot.id, 'start_at' => '2026-12-02T08:00').execute
    assert_equal '2026-12-02T08:00:00Z', @bot.reload.start_at
  end

  test 'a value outside the page choices is refused by the schema' do
    response = UpdateBotSettingsTool.from_wire('bot_id' => @bot.id, 'interval' => 'fortnight').execute

    assert response.error?, response.to_h.inspect
    assert_equal 'day', @bot.reload.interval
  end
end
