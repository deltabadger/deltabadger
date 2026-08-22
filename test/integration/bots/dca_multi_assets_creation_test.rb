require 'test_helper'

class Bots::DcaMultiAssetsCreationTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true)
    @user = create(:user, setup_completed: true)
    @exchange = create(:binance_exchange)
    @bitcoin = create(:asset, :bitcoin)
    @ethereum = create(:asset, :ethereum)
    @solana = create(:asset, symbol: 'SOL', name: 'Solana', external_id: 'solana')
    @usd = create(:asset, :usd)
    [@bitcoin, @ethereum, @solana].each do |asset|
      create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: @usd)
    end
    create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)

    sign_in @user
    Bot::ActionJob.stubs(:perform_later)
  end

  test 'creates a bot when completing all wizard steps' do
    pick @bitcoin
    pick @ethereum
    pick @solana

    advance
    assert_redirected_to new_bots_dca_multi_assets_pick_exchange_path
    post bots_dca_multi_assets_pick_exchange_path,
         params: { bots_dca_multi_asset: { exchange_id: @exchange.id } }
    assert_redirected_to new_bots_dca_multi_assets_add_api_key_path
    follow_redirect!
    assert_redirected_to new_bots_dca_multi_assets_pick_spendable_asset_path

    assert_difference 'Bots::DcaMultiAsset.count', 1 do
      post bots_dca_multi_assets_pick_spendable_asset_path,
           params: { bots_dca_multi_asset: { quote_asset_id: @usd.id } }, as: :turbo_stream
    end

    bot = Bots::DcaMultiAsset.last
    assert_predicate bot, :created?
    assert_equal 'BTC, ETH, SOL', bot.label
    assert_equal [@bitcoin.id, @ethereum.id, @solana.id], bot.base_asset_ids
    assert_in_delta 1, bot.allocations.values.sum, 0.0001
    assert_equal [0.3334, 0.3333, 0.3333], bot.allocations.values
    assert_equal 3, bot.bot_index_assets.in_index.count
    assert_match %(action="redirect" target="#{bot_path(bot)}"), response.body
  end

  test 'a basket of one does not satisfy the multi exchange step' do
    pick @bitcoin

    get new_bots_dca_multi_assets_pick_exchange_path

    assert_redirected_to new_bots_dca_single_assets_pick_buyable_asset_path
  end

  test 'removing down to one asset keeps the step and Next continues as a single bot' do
    pick @bitcoin
    pick @ethereum

    remove @ethereum

    assert_redirected_to new_bots_dca_single_assets_pick_buyable_asset_path
    assert_equal @bitcoin.id, session[:bot_config].dig('settings', 'base_asset_id')
    assert_nil session[:bot_config].dig('settings', 'base_asset_ids')

    advance
    assert_redirected_to new_bots_dca_single_assets_pick_exchange_path
  end

  test 'the asset step reads Buy with the chosen chips and rows; later steps show the basket as one link back' do
    pick @bitcoin
    pick @ethereum

    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_response :ok
    assert_select '.conversational__lead', text: 'Buy'
    assert_select '.conversational__lead', text: /Invest/, count: 0
    assert_select 'div.conversational__stack .ticker', count: 2
    assert_select 'a.conversational__stack', count: 0
    assert_select '.wizard-assets__row', count: 2

    advance
    follow_redirect!
    assert_response :ok
    assert_select 'a.conversational__stack[href=?]', new_bots_dca_single_assets_pick_buyable_asset_path do
      assert_select '.ticker', count: 2
    end
    assert_select '.wizard-asset-list', count: 0
  end

  private

  def pick(asset)
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: asset.id } }
  end

  def remove(asset)
    post remove_bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: asset.id } }
  end

  def advance = post advance_bots_dca_single_assets_pick_buyable_asset_path
end
