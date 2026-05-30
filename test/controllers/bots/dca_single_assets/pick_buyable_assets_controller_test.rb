require 'test_helper'

class Bots::DcaSingleAssets::PickBuyableAssetsControllerTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true) # platform requires an admin to exist before bot flows render
    @user = create(:user, setup_completed: true)
    @binance = create(:binance_exchange)
    @usd = create(:asset, :usd)
    sign_in @user
  end

  test 'lists an available base asset with the exchanges it trades on' do
    kraken = create(:kraken_exchange)
    eth = create(:asset, :ethereum)
    create(:ticker, exchange: @binance, base_asset: eth, quote_asset: @usd)
    create(:ticker, exchange: kraken, base_asset: eth, quote_asset: @usd)

    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_response :ok
    assert_match 'ETH', response.body
    assert_match 'title="Binance"', response.body
    assert_match 'title="Kraken"', response.body
  end

  # The binance_us → binance collapse must survive page-deferred exchange resolution:
  # binance_name has to come from "is Binance available with assets", NOT from the page rows
  # (a page can legitimately contain a binance_us asset but no binance asset).
  test 'collapses binance_us to Binance even when no Binance asset is on the page' do
    # Binance still has an available exchange_asset (so the collapse label resolves) but it is
    # not a tradeable base, so it never appears in the list.
    filler = create(:asset, symbol: 'FILL', name: 'Filler')
    create(:exchange_asset, exchange: @binance, asset: filler, available: true)

    binance_us = create(:binance_us_exchange)
    aaa = create(:asset, symbol: 'AAA', name: 'Alpha')
    create(:ticker, exchange: binance_us, base_asset: aaa, quote_asset: @usd)

    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_response :ok
    assert_match 'AAA', response.body
    assert_match 'title="Binance"', response.body, 'binance_us should render under the Binance label'
    assert_no_match 'Binance.US', response.body
  end

  test 'paginates the asset list in pages of ASSET_PAGE_SIZE' do
    page_size = Bots::Searchable::ASSET_PAGE_SIZE
    total = page_size + 5
    total.times do |i|
      asset = create(:asset, symbol: "C#{i}", name: "Coin #{i}")
      create(:ticker, exchange: @binance, base_asset: asset, quote_asset: @usd)
    end

    get new_bots_dca_single_assets_pick_buyable_asset_path
    assert_response :ok
    first_page = response.body.scan('data-arrow-keys-navigation-target="item"').size
    assert_equal page_size, first_page

    get new_bots_dca_single_assets_pick_buyable_asset_path(offset: page_size)
    assert_response :ok
    second_page = response.body.scan('data-arrow-keys-navigation-target="item"').size
    assert_equal 5, second_page
  end
end
