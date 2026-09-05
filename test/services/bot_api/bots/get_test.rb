# frozen_string_literal: true

require 'test_helper'

class BotApi::Bots::GetTest < ActiveSupport::TestCase
  setup { @user = create(:user) }

  test 'a composition bot reports its exited holdings and redeploy offer' do
    bot = create(:dca_index, user: @user, status: :stopped)
    Bots::DcaIndex.any_instance.stubs(:exited_symbols).returns(%w[DOGE SHIB])
    Bots::DcaIndex.any_instance.stubs(:redeploy_offer).returns(25.5.to_d)

    data = BotApi::Bots::Get.call(user: @user, bot_id: bot.id).data

    assert_equal %w[DOGE SHIB], data[:exited_holdings]
    assert_equal '25.5', data[:redeploy_offer]
  end

  test 'a single-asset bot reports neither' do
    bot = create(:dca_single_asset, :stopped, user: @user)
    data = BotApi::Bots::Get.call(user: @user, bot_id: bot.id).data
    assert_nil data[:exited_holdings]
    assert_nil data[:redeploy_offer]
  end

  test 'a failing offer or holdings read does not fail the call' do
    bot = create(:dca_index, user: @user, status: :stopped)
    Bots::DcaIndex.any_instance.stubs(:redeploy_offer).raises(StandardError, 'cold cache')
    Bots::DcaIndex.any_instance.stubs(:exited_symbols).raises(StandardError, 'cold cache')
    result = BotApi::Bots::Get.call(user: @user, bot_id: bot.id)
    assert result.success?
    assert_nil result.data[:exited_holdings]
    assert_nil result.data[:redeploy_offer]
  end

  test 'a zero offer is reported as zero, not as absent' do
    bot = create(:dca_index, user: @user, status: :stopped)
    Bots::DcaIndex.any_instance.stubs(:redeploy_offer).returns(0.to_d)

    assert_equal '0', BotApi::Bots::Get.call(user: @user, bot_id: bot.id).data[:redeploy_offer]
  end
end
