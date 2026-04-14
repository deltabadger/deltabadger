require 'test_helper'

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

  # Dual-asset creation starts in the single-asset flow and is promoted to dual via the "+" button.
  test 'creates a bot when completing all wizard steps' do
    # Step 1: Pick first asset (via unified single-asset entry)
    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_response :ok

    post bots_dca_single_assets_pick_buyable_asset_path, params: {
      bots_dca_single_asset: { base_asset_id: @bitcoin.id }
    }
    assert_redirected_to new_bots_dca_single_assets_pick_exchange_path
    follow_redirect!

    # Step 1b: Promote to dual via "+"
    post promote_to_dual_bots_dca_single_assets_pick_exchange_path
    assert_redirected_to new_bots_dca_dual_assets_pick_second_buyable_asset_path
    follow_redirect!

    # Step 2: Pick second asset
    assert_response :ok

    post bots_dca_dual_assets_pick_second_buyable_asset_path, params: {
      bots_dca_dual_asset: { base1_asset_id: @ethereum.id }
    }
    assert_redirected_to new_bots_dca_dual_assets_pick_exchange_path
    follow_redirect!

    # Step 3: Pick exchange
    assert_response :ok

    post bots_dca_dual_assets_pick_exchange_path, params: {
      bots_dca_dual_asset: { exchange_id: @exchange.id }
    }
    assert_redirected_to new_bots_dca_dual_assets_add_api_key_path
    follow_redirect!

    # Step 4: API key (already validated, should redirect)
    assert_redirected_to new_bots_dca_dual_assets_pick_spendable_asset_path
    follow_redirect!

    # Step 5: Pick spendable asset
    assert_response :ok

    # Picking the spendable asset persists the bot in :created state with defaults —
    # no separate confirm step.
    assert_difference 'Bots::DcaDualAsset.count', 1 do
      post bots_dca_dual_assets_pick_spendable_asset_path, params: {
        bots_dca_dual_asset: { quote_asset_id: @usd.id }
      }, as: :turbo_stream
    end
    assert_response :ok

    bot = Bots::DcaDualAsset.last
    assert_equal @bitcoin, bot.base0_asset
    assert_equal @ethereum, bot.base1_asset
    assert_equal @usd, bot.quote_asset
    assert_equal @exchange, bot.exchange
    assert_equal 100, bot.quote_amount
    assert_equal 'week', bot.interval
    assert_equal 0.5, bot.allocation0
    assert_predicate bot, :created?
    assert_match %(action="redirect" target="#{bot_path(bot)}"), response.body
  end

  test 'redirects to single-asset picker when accessing second asset step without a first asset' do
    get new_bots_dca_dual_assets_pick_second_buyable_asset_path
    assert_redirected_to new_bots_dca_single_assets_pick_buyable_asset_path
  end

  test 'requires authentication for wizard' do
    sign_out @user

    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_redirected_to new_user_session_path
  end
end
