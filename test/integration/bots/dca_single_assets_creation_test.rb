require 'test_helper'

class Bots::DcaSingleAssetsCreationTest < ActionDispatch::IntegrationTest
  setup do
    @admin = create(:user, admin: true)
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @bitcoin = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    @ticker = create(:ticker, exchange: @exchange, base_asset: @bitcoin, quote_asset: @usd)
    @api_key = create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)

    sign_in @user
    Bot::ActionJob.stubs(:perform_later)
  end

  test 'creates a bot when completing all wizard steps' do
    # Step 1: Pick buyable asset
    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_response :ok

    post bots_dca_single_assets_pick_buyable_asset_path, params: {
      bots_dca_single_asset: { base_asset_id: @bitcoin.id }
    }
    assert_redirected_to new_bots_dca_single_assets_pick_exchange_path
    follow_redirect!

    # Step 2: Pick exchange
    assert_response :ok

    post bots_dca_single_assets_pick_exchange_path, params: {
      bots_dca_single_asset: { exchange_id: @exchange.id }
    }
    assert_redirected_to new_bots_dca_single_assets_add_api_key_path
    follow_redirect!

    # Step 3: API key (already validated, should redirect)
    assert_redirected_to new_bots_dca_single_assets_pick_spendable_asset_path
    follow_redirect!

    # Step 4: Pick spendable asset
    assert_response :ok

    post bots_dca_single_assets_pick_spendable_asset_path, params: {
      bots_dca_single_asset: { quote_asset_id: @usd.id }
    }
    assert_redirected_to new_bots_dca_single_assets_confirm_settings_path
    follow_redirect!

    # Step 5: Confirm settings
    assert_response :ok

    post bots_dca_single_assets_confirm_settings_path, params: {
      bots_dca_single_asset: { quote_amount: 100, interval: 'day' }
    }, as: :turbo_stream
    assert_response :ok

    # Step 6: Create bot
    assert_difference 'Bots::DcaSingleAsset.count', 1 do
      post bots_dca_single_assets_path, as: :turbo_stream
    end

    bot = Bots::DcaSingleAsset.last
    assert_equal @bitcoin, bot.base_asset
    assert_equal @usd, bot.quote_asset
    assert_equal @exchange, bot.exchange
    assert_equal 100, bot.quote_amount
    assert_predicate bot, :scheduled?
  end

  test 'creates bot with weekly interval' do
    get new_bots_dca_single_assets_pick_buyable_asset_path

    post bots_dca_single_assets_pick_buyable_asset_path, params: {
      bots_dca_single_asset: { base_asset_id: @bitcoin.id }
    }
    follow_redirect!

    post bots_dca_single_assets_pick_exchange_path, params: {
      bots_dca_single_asset: { exchange_id: @exchange.id }
    }
    follow_redirect!
    follow_redirect! # skip API key

    post bots_dca_single_assets_pick_spendable_asset_path, params: {
      bots_dca_single_asset: { quote_asset_id: @usd.id }
    }
    follow_redirect!

    post bots_dca_single_assets_confirm_settings_path, params: {
      bots_dca_single_asset: { quote_amount: 50, interval: 'week' }
    }, as: :turbo_stream

    assert_difference 'Bots::DcaSingleAsset.count', 1 do
      post bots_dca_single_assets_path, as: :turbo_stream
    end

    bot = Bots::DcaSingleAsset.last
    assert_equal 50, bot.quote_amount
    assert_equal 'week', bot.interval
  end

  test 'redirects to pick asset when accessing exchange step directly' do
    get new_bots_dca_single_assets_pick_exchange_path
    assert_redirected_to new_bots_dca_single_assets_pick_buyable_asset_path
  end

  test 'requires authentication for wizard' do
    sign_out @user

    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_redirected_to new_user_session_path
  end

  test 'requires authentication to create bot' do
    sign_out @user

    post bots_dca_single_assets_path, as: :turbo_stream
    assert_redirected_to new_user_session_path
  end
end
