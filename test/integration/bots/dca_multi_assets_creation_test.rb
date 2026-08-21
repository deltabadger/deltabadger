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
    promote(@bitcoin)
    add_asset(@ethereum)
    add_asset(@solana)

    post advance_bots_dca_multi_assets_pick_assets_path
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

  test 'a list of one does not satisfy the assets step' do
    promote(@bitcoin)

    get new_bots_dca_multi_assets_pick_exchange_path

    assert_redirected_to new_bots_dca_multi_assets_pick_assets_path
  end

  test 'removing down to one asset demotes to the single flow' do
    promote(@bitcoin)
    add_asset(@ethereum)

    post remove_bots_dca_multi_assets_pick_assets_path,
         params: { bots_dca_multi_asset: { base_asset_id: @ethereum.id } }

    assert_redirected_to new_bots_dca_single_assets_pick_spendable_asset_path
    assert_equal @bitcoin.id, session[:bot_config].dig('settings', 'base_asset_id')
    assert_nil session[:bot_config].dig('settings', 'base_asset_ids')
  end

  test 'an asset outside the search scope, a duplicate, and the twenty-first are ignored' do
    promote(@bitcoin)
    outside = create(:asset, symbol: 'OUT', name: 'Outside', external_id: 'outside')
    add_asset(outside)
    assert_equal [@bitcoin.id], chosen_ids

    add_asset(@bitcoin)
    assert_equal [@bitcoin.id], chosen_ids

    additions = 20.times.map do |index|
      asset = create(:asset, symbol: "A#{index}", name: "Asset #{index}", external_id: "asset-#{index}")
      create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: @usd)
      asset
    end
    additions.first(19).each { |asset| add_asset(asset) }
    assert_equal Bots::DcaMultiAsset::MAX_ASSETS, chosen_ids.size

    add_asset(additions.last)
    assert_equal Bots::DcaMultiAsset::MAX_ASSETS, chosen_ids.size
    refute_includes chosen_ids, additions.last.id
  end

  test 'advance with one asset stays put' do
    promote(@bitcoin)

    post advance_bots_dca_multi_assets_pick_assets_path

    assert_redirected_to new_bots_dca_multi_assets_pick_assets_path
  end

  test 'an empty session GET bounces to the single picker' do
    get new_bots_dca_multi_assets_pick_assets_path

    assert_redirected_to new_bots_dca_single_assets_pick_buyable_asset_path
  end

  test 'the assets step keeps the sentence compact and renders every chosen row' do
    promote(@bitcoin)
    add_asset(@ethereum)

    get new_bots_dca_multi_assets_pick_assets_path

    assert_response :ok
    assert_select '.conversational__lead', text: /Invest/
    assert_select '.conversational__assets .ticker.filled', count: 0
    assert_select '.wizard-assets__row', count: 2
  end

  private

  def promote(asset)
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: asset.id } }
    post promote_to_multi_bots_dca_single_assets_pick_exchange_path
  end

  def add_asset(asset)
    post bots_dca_multi_assets_pick_assets_path,
         params: { bots_dca_multi_asset: { base_asset_id: asset.id } }
  end

  def chosen_ids
    session[:bot_config].dig('settings', 'base_asset_ids')
  end
end
