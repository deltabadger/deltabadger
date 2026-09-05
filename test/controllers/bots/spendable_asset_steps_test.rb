require 'test_helper'

# B5. The asset step's rows and its exchange logos now come from one relation per bot type
# (Bot#offered_tickers). Index and multi-asset bots have no base_asset_id store_accessor, so a
# single shared implementation reading one would raise NoMethodError on exactly these two screens —
# which had no controller coverage at all. These render the steps for both types.
#
# The index step renders a quote_asset_result partial rather than the exchange-chip cell, so the
# meaningful assertion there is that it renders; the multi step does render chips.
class Bots::SpendableAssetStepsTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true)
    @user = create(:user, setup_completed: true)
    sign_in @user
    configure_deltabadger_market_data

    @binance = create(:binance_exchange)
    @usd = create(:asset, :usd)
    @coins = %w[bitcoin ethereum solana].map do |id|
      asset = create(:asset, symbol: id[0..2].upcase, name: id, external_id: id)
      create(:ticker, exchange: @binance, base_asset: asset, quote_asset: @usd)
      asset
    end
  end

  test 'the index asset step renders' do
    Index.create!(external_id: 'layer-1', source: Index::SOURCE_COINGECKO, name: 'Layer 1', weight: 100,
                  top_coins: @coins.map(&:external_id),
                  top_coins_by_exchange: { 'Exchanges::Binance' => @coins.map(&:external_id) },
                  available_exchanges: { 'Exchanges::Binance' => 3 })
    post bots_dca_indexes_pick_index_path,
         params: { index_type: Bots::DcaIndex::INDEX_TYPE_CATEGORY,
                   index_category_id: 'layer-1', index_name: 'Layer 1' }
    post bots_dca_indexes_pick_exchange_path,
         params: { bots_dca_index: { exchange_id: @binance.id } }

    get new_bots_dca_indexes_pick_spendable_asset_path
    assert_response :ok
  end

  test 'the multi-asset asset step renders its exchange chips' do
    post bots_dca_single_assets_order_path, params: { flow: 'asset_first' }
    @coins.first(2).each do |asset|
      post bots_dca_single_assets_pick_buyable_asset_path,
           params: { bots_dca_single_asset: { base_asset_id: asset.id } }
    end
    post advance_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_multi_assets_pick_exchange_path,
         params: { bots_dca_multi_asset: { exchange_id: @binance.id } }

    get new_bots_dca_multi_assets_pick_spendable_asset_path
    assert_response :ok
    assert_match 'title="Binance"', response.body
  end

  # B6. Exchange.tradeable, not Exchange.available, is what keeps a retired venue out of the basket
  # picker. A refactor that flattened the three bot types onto one scope would re-admit it.
  test 'the multi-asset step does not offer a retired venue' do
    retired = create(:bitmart_exchange)
    @coins.each { |c| create(:ticker, exchange: retired, base_asset: c, quote_asset: @usd) }

    post bots_dca_single_assets_order_path, params: { flow: 'asset_first' }
    @coins.first(2).each do |asset|
      post bots_dca_single_assets_pick_buyable_asset_path,
           params: { bots_dca_single_asset: { base_asset_id: asset.id } }
    end
    post advance_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_multi_assets_pick_exchange_path,
         params: { bots_dca_multi_asset: { exchange_id: @binance.id } }

    get new_bots_dca_multi_assets_pick_spendable_asset_path
    assert_response :ok
    assert_no_match 'title="BitMart"', response.body
  end
end
