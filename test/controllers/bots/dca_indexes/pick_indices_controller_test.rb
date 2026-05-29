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
end
