require 'test_helper'

class Bots::DcaIndexes::PickExchangesControllerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    MarketDataSettings.stubs(:current_provider).returns(MarketDataSettings::PROVIDER_DELTABADGER)

    @alpaca = create(:alpaca_exchange)
    @binance = create(:binance_exchange)

    # A deltabadger-sourced stock index whose members are only on Alpaca.
    Index.create!(external_id: 'nasdaq-100', source: Index::SOURCE_DELTABADGER, name: 'Nasdaq 100',
                  weight: 100, available_exchanges: { 'Exchanges::Alpaca' => 3 })
  end

  test 'restricts exchanges to the deltabadger index available_exchanges (Alpaca only)' do
    # Seed the wizard session: choose the Nasdaq category index.
    post bots_dca_indexes_pick_index_path,
         params: { index_type: Bots::DcaIndex::INDEX_TYPE_CATEGORY,
                   index_category_id: 'nasdaq-100', index_name: 'Nasdaq 100' }

    get new_bots_dca_indexes_pick_exchange_path
    assert_response :success

    # Exchanges render as submit buttons valued by exchange.id. With the source-agnostic lookup, only
    # Alpaca (the index's available_exchanges) is offered — not every crypto exchange. Pre-fix the
    # deltabadger index resolved to nil and fell back to Exchange.available, which included Binance.
    assert_match(/value="#{@alpaca.id}"/, response.body)
    assert_no_match(/value="#{@binance.id}"/, response.body)
  end
end
