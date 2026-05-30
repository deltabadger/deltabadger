require 'test_helper'

class AssetsControllerTest < ActionDispatch::IntegrationTest
  # GET /asset_tooltip?symbol=BTC — resolves a .ticker pill's symbol to an asset and renders
  # the hover-card (logo, name, type label, current price). Gated on the data API being
  # connected; fail-soft on price (non-crypto / flaky upstream → no price, never a 500).

  setup do
    create(:user, admin: true) # platform requires an admin to exist before authed pages render
    @user = create(:user, setup_completed: true)
    sign_in @user
    create(:asset, :bitcoin, image_url: 'https://img/btc.png')
    MarketDataSettings.stubs(:deltabadger?).returns(true)
  end

  test 'renders the asset card for a known symbol' do
    Asset.any_instance.stubs(:get_price).returns(Result::Success.new(67_123.45))

    get asset_tooltip_path(symbol: 'BTC')

    assert_response :ok
    assert_includes response.body, 'Bitcoin'
    assert_includes response.body, 'BTC'
    assert_includes response.body, 'Crypto'        # type label
    assert_includes response.body, '67,123.45'     # formatted price
  end

  test 'symbol match is case-insensitive' do
    Asset.any_instance.stubs(:get_price).returns(Result::Failure.new('x'))

    get asset_tooltip_path(symbol: 'btc')

    assert_response :ok
    assert_includes response.body, 'Bitcoin'
  end

  test 'when several assets share a symbol it picks the highest market cap (lowest rank)' do
    create(:asset, symbol: 'SUN', name: 'Low Cap Sun', market_cap_rank: nil)
    create(:asset, symbol: 'SUN', name: 'Big Sun', market_cap_rank: 5)
    Asset.any_instance.stubs(:get_price).returns(Result::Failure.new('x'))

    get asset_tooltip_path(symbol: 'SUN')

    assert_response :ok
    assert_includes response.body, 'Big Sun'
    refute_includes response.body, 'Low Cap Sun'
  end

  test 'a non-crypto asset renders the card without a price (no error)' do
    create(:asset, :usd) # Fiat → get_price fails
    Asset.any_instance.stubs(:get_price).returns(Result::Failure.new('not a cryptocurrency'))

    get asset_tooltip_path(symbol: 'USD')

    assert_response :ok
    assert_includes response.body, 'US Dollar'
  end

  test 'a transient upstream price failure is rescued, never a 500' do
    Asset.any_instance.stubs(:get_price).raises(Client::TransientNetworkError, 'boom')

    get asset_tooltip_path(symbol: 'BTC')

    assert_response :ok
    assert_includes response.body, 'Bitcoin'
  end

  test 'is inert (404) when the data API is not connected' do
    MarketDataSettings.unstub(:deltabadger?)
    MarketDataSettings.stubs(:deltabadger?).returns(false)

    get asset_tooltip_path(symbol: 'BTC')

    assert_response :not_found
  end

  test 'returns 404 for an unknown symbol' do
    get asset_tooltip_path(symbol: 'NOPE')

    assert_response :not_found
  end

  test 'returns 404 for a blank symbol' do
    get asset_tooltip_path(symbol: '')

    assert_response :not_found
  end

  test 'requires authentication' do
    sign_out @user

    get asset_tooltip_path(symbol: 'BTC')

    assert_redirected_to new_user_session_path
  end
end
