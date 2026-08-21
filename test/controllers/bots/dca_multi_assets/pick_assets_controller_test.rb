require 'test_helper'

class Bots::DcaMultiAssets::PickAssetsControllerTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true)
    @user = create(:user, setup_completed: true)
    @exchange = create(:binance_exchange)
    @usd = create(:asset, :usd)
    @assets = 22.times.map do |index|
      asset = create(:asset, symbol: "A#{index}", name: "Asset #{index}", external_id: "asset-#{index}")
      create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: @usd)
      asset
    end
    sign_in @user
  end

  test 'with twenty assets the step renders the cap note, no picker, and a Next button' do
    promote_first
    @assets.drop(1).first(19).each { |asset| add_asset(asset) }

    get new_bots_dca_multi_assets_pick_assets_path

    assert_response :ok
    assert_select '.modal--search__input', count: 0
    assert_select '.text-inactive', text: I18n.t('bot.dca_multi_asset.max_assets_reached', max: 20)
    assert_select 'button', text: I18n.t('button.next') do |buttons|
      assert_nil buttons.first['disabled']
    end
  end

  test 'a lazy page frame request still renders just the page partial' do
    promote_first

    get new_bots_dca_multi_assets_pick_assets_path,
        params: { offset: 20 }, headers: { 'Turbo-Frame' => 'assets-page-20' }

    assert_response :ok
    assert_select 'turbo-frame#assets-page-20'
    assert_select '.bot-creation-layout', count: 0
  end

  private

  def promote_first
    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: @assets.first.id } }
    post promote_to_multi_bots_dca_single_assets_pick_exchange_path
  end

  def add_asset(asset)
    post bots_dca_multi_assets_pick_assets_path,
         params: { bots_dca_multi_asset: { base_asset_id: asset.id } }
  end
end
