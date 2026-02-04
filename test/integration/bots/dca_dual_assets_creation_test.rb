require "test_helper"

class Bots::DcaDualAssetsCreationTest < ActionDispatch::IntegrationTest
  setup do
    @admin = create(:user, admin: true)
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @bitcoin = create(:asset, :bitcoin)
    @ethereum = create(:asset, :ethereum)
    @usd = create(:asset, :usd)
    @btc_ticker = create(:ticker, exchange: @exchange, base_asset: @bitcoin, quote_asset: @usd)
    @eth_ticker = create(:ticker, exchange: @exchange, base_asset: @ethereum, quote_asset: @usd)
    @api_key = create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)

    sign_in @user
    Bot::ActionJob.stubs(:perform_later)
  end

  test "creates a bot when completing all wizard steps" do
    # Step 1: Pick first asset
    get new_bots_dca_dual_assets_pick_first_buyable_asset_path
    assert_response :ok

    post bots_dca_dual_assets_pick_first_buyable_asset_path, params: {
      bots_dca_dual_asset: {base0_asset_id: @bitcoin.id}
    }
    assert_redirected_to new_bots_dca_dual_assets_pick_second_buyable_asset_path
    follow_redirect!

    # Step 2: Pick second asset
    assert_response :ok

    post bots_dca_dual_assets_pick_second_buyable_asset_path, params: {
      bots_dca_dual_asset: {base1_asset_id: @ethereum.id}
    }
    assert_redirected_to new_bots_dca_dual_assets_pick_exchange_path
    follow_redirect!

    # Step 3: Pick exchange
    assert_response :ok

    post bots_dca_dual_assets_pick_exchange_path, params: {
      bots_dca_dual_asset: {exchange_id: @exchange.id}
    }
    assert_redirected_to new_bots_dca_dual_assets_add_api_key_path
    follow_redirect!

    # Step 4: API key (already validated, should redirect)
    assert_redirected_to new_bots_dca_dual_assets_pick_spendable_asset_path
    follow_redirect!

    # Step 5: Pick spendable asset
    assert_response :ok

    post bots_dca_dual_assets_pick_spendable_asset_path, params: {
      bots_dca_dual_asset: {quote_asset_id: @usd.id}
    }
    assert_redirected_to new_bots_dca_dual_assets_confirm_settings_path
    follow_redirect!

    # Step 6: Confirm settings
    assert_response :ok

    post bots_dca_dual_assets_confirm_settings_path, params: {
      bots_dca_dual_asset: {quote_amount: 200, interval: "day", allocation0: 0.6}
    }, as: :turbo_stream
    assert_response :ok

    # Step 7: Create bot
    assert_difference "Bots::DcaDualAsset.count", 1 do
      post bots_dca_dual_assets_path, as: :turbo_stream
    end

    bot = Bots::DcaDualAsset.last
    assert_equal @bitcoin, bot.base0_asset
    assert_equal @ethereum, bot.base1_asset
    assert_equal @usd, bot.quote_asset
    assert_equal @exchange, bot.exchange
    assert_equal 200, bot.quote_amount
    assert_equal 0.6, bot.allocation0
    assert_predicate bot, :scheduled?
  end

  test "creates bot with custom allocation" do
    get new_bots_dca_dual_assets_pick_first_buyable_asset_path

    post bots_dca_dual_assets_pick_first_buyable_asset_path, params: {
      bots_dca_dual_asset: {base0_asset_id: @bitcoin.id}
    }
    follow_redirect!

    post bots_dca_dual_assets_pick_second_buyable_asset_path, params: {
      bots_dca_dual_asset: {base1_asset_id: @ethereum.id}
    }
    follow_redirect!

    post bots_dca_dual_assets_pick_exchange_path, params: {
      bots_dca_dual_asset: {exchange_id: @exchange.id}
    }
    follow_redirect!
    follow_redirect! # skip API key

    post bots_dca_dual_assets_pick_spendable_asset_path, params: {
      bots_dca_dual_asset: {quote_asset_id: @usd.id}
    }
    follow_redirect!

    post bots_dca_dual_assets_confirm_settings_path, params: {
      bots_dca_dual_asset: {quote_amount: 100, interval: "week", allocation0: 0.8}
    }, as: :turbo_stream

    assert_difference "Bots::DcaDualAsset.count", 1 do
      post bots_dca_dual_assets_path, as: :turbo_stream
    end

    bot = Bots::DcaDualAsset.last
    assert_equal 0.8, bot.allocation0
    assert_equal "week", bot.interval
  end

  test "redirects to first asset when accessing second asset step directly" do
    get new_bots_dca_dual_assets_pick_second_buyable_asset_path
    assert_redirected_to new_bots_dca_dual_assets_pick_first_buyable_asset_path
  end

  test "requires authentication for wizard" do
    sign_out @user

    get new_bots_dca_dual_assets_pick_first_buyable_asset_path
    assert_redirected_to new_user_session_path
  end
end
