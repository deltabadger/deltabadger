require 'test_helper'

class Bots::DcaIndexes::PickIndexMarkupTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user

    # The controller's require_market_data_configured before_action gates on
    # MarketDataSettings.current_provider being present; stubbing it here also
    # satisfies that gate, matching how the sibling controller test does it.
    MarketDataSettings.stubs(:current_provider).returns(MarketDataSettings::PROVIDER_COINGECKO)

    @index = create(:index, external_id: 'defi', name: 'DeFi', source: Index::SOURCE_COINGECKO)
  end

  test 'index tiles carry their identity as data attributes, not an inline handler' do
    get new_bots_dca_indexes_pick_index_path

    assert_response :success
    assert_select 'button[data-index-category-id=?][data-index-name=?][data-action=?]',
                  @index.external_id, @index.name, 'click->index-filter#select'
    assert_select 'button[onclick]', false, 'an inline handler here fails silently, not visibly'
    assert_select '#index_category_id[data-index-filter-target=?]', 'categoryIdField'
    assert_select '#index_name[data-index-filter-target=?]', 'nameField'
  end
end
