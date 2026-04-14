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

  # Wizard ends at pick_spendable_asset — the bot is persisted with status: :created
  # and sensible defaults (quote_amount: 100, interval: 'week'); the user edits
  # and starts the bot from the show page.
  test 'creates an unstarted bot when completing the wizard' do
    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_response :ok

    post bots_dca_single_assets_pick_buyable_asset_path, params: {
      bots_dca_single_asset: { base_asset_id: @bitcoin.id }
    }
    assert_redirected_to new_bots_dca_single_assets_pick_exchange_path
    follow_redirect!

    post bots_dca_single_assets_pick_exchange_path, params: {
      bots_dca_single_asset: { exchange_id: @exchange.id }
    }
    assert_redirected_to new_bots_dca_single_assets_add_api_key_path
    follow_redirect!
    # API key pre-validated → short-circuits to pick_spendable_asset
    assert_redirected_to new_bots_dca_single_assets_pick_spendable_asset_path
    follow_redirect!
    assert_response :ok

    assert_difference 'Bots::DcaSingleAsset.count', 1 do
      post bots_dca_single_assets_pick_spendable_asset_path, params: {
        bots_dca_single_asset: { quote_asset_id: @usd.id }
      }, as: :turbo_stream
    end
    assert_response :ok

    bot = Bots::DcaSingleAsset.last
    assert_equal @bitcoin, bot.base_asset
    assert_equal @usd, bot.quote_asset
    assert_equal @exchange, bot.exchange
    assert_equal 100, bot.quote_amount
    assert_equal 'week', bot.interval
    assert_predicate bot, :created?
    # Turbo stream break-out redirect to the bot show page.
    assert_match %(action="redirect" target="#{bot_path(bot)}"), response.body
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
end
