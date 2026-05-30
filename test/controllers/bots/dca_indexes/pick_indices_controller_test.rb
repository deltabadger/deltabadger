require 'test_helper'

class Bots::DcaIndexes::PickIndicesControllerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user

    @coingecko_index = Index.create!(external_id: 'layer-1', source: Index::SOURCE_COINGECKO,
                                     name: 'Layer 1', weight: 12)
    @nasdaq_index = Index.create!(external_id: 'nasdaq-100', source: Index::SOURCE_DELTABADGER,
                                  name: 'Nasdaq 100', weight: 100)
  end

  test 'shows the Nasdaq (deltabadger) index when the Data API is the provider' do
    MarketDataSettings.stubs(:current_provider).returns(MarketDataSettings::PROVIDER_DELTABADGER)

    get new_bots_dca_indexes_pick_index_path
    assert_response :success
    assert_match 'Nasdaq 100', response.body
    assert_match 'Layer 1', response.body
  end

  test 'hides deltabadger-source indices when running on CoinGecko-direct' do
    MarketDataSettings.stubs(:current_provider).returns(MarketDataSettings::PROVIDER_COINGECKO)

    get new_bots_dca_indexes_pick_index_path
    assert_response :success
    assert_no_match 'Nasdaq 100', response.body
    assert_match 'Layer 1', response.body
  end

  test 'orders stock (deltabadger) indices first, then internal Top Coins, then coingecko' do
    Index.create!(external_id: Index::TOP_COINS_EXTERNAL_ID, source: Index::SOURCE_INTERNAL,
                  name: 'Top Coins', weight: 50)
    MarketDataSettings.stubs(:current_provider).returns(MarketDataSettings::PROVIDER_DELTABADGER)

    get new_bots_dca_indexes_pick_index_path
    assert_response :success

    nasdaq_pos = response.body.index('Nasdaq 100')
    top_pos = response.body.index('Top Coins')
    layer_pos = response.body.index('Layer 1')
    assert nasdaq_pos && top_pos && layer_pos, 'all three index tiles should render'
    assert nasdaq_pos < top_pos, 'stock (deltabadger) index should sort before internal Top Coins'
    assert top_pos < layer_pos, 'internal Top Coins should sort before a coingecko category'
  end

  test 'selecting a count-named index stores its name prefix; switching to Top Coins clears it' do
    MarketDataSettings.stubs(:current_provider).returns(MarketDataSettings::PROVIDER_DELTABADGER)

    post bots_dca_indexes_pick_index_path,
         params: { index_type: 'category', index_category_id: 'nasdaq-100', index_name: 'Nasdaq 20' }
    assert_equal 'Nasdaq', session[:bot_config]['settings']['index_name_prefix']

    post bots_dca_indexes_pick_index_path, params: { index_type: 'top' }
    assert_nil session[:bot_config]['settings']['index_name_prefix'],
               'stale prefix would otherwise render a Top Coins bot as "Nasdaq N"'
  end

  test 'keeps the provider filter on the invalid-submit re-render (CoinGecko-direct)' do
    MarketDataSettings.stubs(:current_provider).returns(MarketDataSettings::PROVIDER_COINGECKO)

    # No index_type/category → falls into the error branch which re-renders the picker.
    post bots_dca_indexes_pick_index_path, params: {}
    assert_response :unprocessable_entity
    assert_no_match 'Nasdaq 100', response.body
    assert_match 'Layer 1', response.body
  end
end
