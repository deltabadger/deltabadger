require 'test_helper'

# Every DCA bot the wizard creates is a basket. One asset continues as a one-asset Bots::DcaMultiAsset,
# which does everything the single-asset bot did; no step saves a Bots::DcaSingleAsset any more.
class Bots::OneAssetCreationTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true)
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @bitcoin = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    create(:ticker, exchange: @exchange, base_asset: @bitcoin, quote_asset: @usd)
    create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)

    sign_in @user
    Bot::ActionJob.stubs(:perform_later)
  end

  # The default order: the venue first, then the asset, then the quote. The bot is saved unstarted with
  # the wizard defaults; the user starts it from its page.
  test 'exchange-first: one asset is saved as a one-asset basket, named after the asset, and starts' do
    get new_bots_dca_single_assets_pick_exchange_path
    assert_response :ok
    post bots_dca_single_assets_pick_exchange_path, params: { bots_dca_single_asset: { exchange_id: @exchange.id } }
    assert_redirected_to new_bots_dca_single_assets_add_api_key_path
    follow_redirect!
    # API key pre-validated → short-circuits to the asset step
    assert_redirected_to new_bots_dca_single_assets_pick_buyable_asset_path

    pick @bitcoin
    advance
    assert_redirected_to new_bots_dca_multi_assets_pick_spendable_asset_path
    follow_redirect!
    assert_response :ok

    assert_no_difference 'Bots::DcaSingleAsset.count' do
      assert_difference 'Bots::DcaMultiAsset.count', 1 do
        post bots_dca_multi_assets_pick_spendable_asset_path,
             params: { bots_dca_multi_asset: { quote_asset_id: @usd.id } }, as: :turbo_stream
      end
    end

    bot = Bots::DcaMultiAsset.last
    assert_predicate bot, :one_asset?
    assert_equal({ @bitcoin.id.to_s => 1.0 }, bot.allocations)
    assert_equal 'Bitcoin', bot.label, 'named after the asset, as the single-asset bot was'
    assert_equal [@bitcoin, @usd, @exchange], [bot.base_asset, bot.quote_asset, bot.exchange]
    assert_equal [100, 'week'], [bot.quote_amount, bot.interval]
    assert_predicate bot, :created?
    assert_match %(action="redirect" target="#{bot_path(bot)}"), response.body

    assert bot.start(start_fresh: true), bot.errors.full_messages.to_sentence
    assert_predicate bot.reload, :scheduled?
  end

  test 'asset-first: one asset continues to the basket exchange step and is saved as a basket' do
    post bots_dca_single_assets_order_path, params: { flow: 'asset_first' }
    pick @bitcoin
    advance
    assert_redirected_to new_bots_dca_multi_assets_pick_exchange_path

    post bots_dca_multi_assets_pick_exchange_path, params: { bots_dca_multi_asset: { exchange_id: @exchange.id } }
    assert_redirected_to new_bots_dca_multi_assets_add_api_key_path
    follow_redirect!
    assert_redirected_to new_bots_dca_multi_assets_pick_spendable_asset_path

    assert_difference 'Bots::DcaMultiAsset.count', 1 do
      post bots_dca_multi_assets_pick_spendable_asset_path,
           params: { bots_dca_multi_asset: { quote_asset_id: @usd.id } }, as: :turbo_stream
    end
    assert_equal [@bitcoin.id], Bots::DcaMultiAsset.last.base_asset_ids
  end

  # A session left open by the previous release holds one asset under the single-asset key. The quote step
  # never sees it as a basket and sends it back to the asset step, whose Next commits it as one.
  test 'the quote step sends a session without a basket back to the asset step' do
    get new_bots_dca_single_assets_pick_exchange_path
    post bots_dca_single_assets_pick_exchange_path, params: { bots_dca_single_asset: { exchange_id: @exchange.id } }

    get new_bots_dca_multi_assets_pick_spendable_asset_path

    assert_redirected_to new_bots_dca_single_assets_pick_buyable_asset_path
  end

  test 'the single-asset quote step and create endpoint are gone' do
    %w[GET POST].each do |verb|
      assert_raises(ActionController::RoutingError) do
        Rails.application.routes.recognize_path('/en/bots/dca_single_assets/pick_spendable_asset', method: verb)
      end
    end
    assert_raises(ActionController::RoutingError) do
      Rails.application.routes.recognize_path('/en/bots/dca_single_assets/pick_spendable_asset/new', method: 'GET')
    end
    assert_raises(ActionController::RoutingError) do
      Rails.application.routes.recognize_path('/en/bots/dca_single_assets', method: 'POST')
    end
  end

  test 'redirects to the exchange step when accessing the asset step directly' do
    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_redirected_to new_bots_dca_single_assets_pick_exchange_path
  end

  test 'requires authentication for wizard' do
    sign_out @user

    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_redirected_to new_user_session_path
  end

  private

  def pick(asset)
    post bots_dca_single_assets_pick_buyable_asset_path, params: { bots_dca_single_asset: { base_asset_id: asset.id } }
  end

  def advance = post advance_bots_dca_single_assets_pick_buyable_asset_path
end
