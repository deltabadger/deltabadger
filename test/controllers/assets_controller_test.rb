require 'test_helper'

class AssetsControllerTest < ActionDispatch::IntegrationTest
  # GET /asset_tooltip?symbol=BTC — resolves a .ticker pill's symbol to an asset and renders
  # the hover-card (logo, name, symbol, type label).

  setup do
    create(:user, admin: true) # platform requires an admin to exist before authed pages render
    @user = create(:user, setup_completed: true)
    sign_in @user
    create(:asset, :bitcoin, image_url: 'https://img/btc.png')
  end

  test 'renders the asset card for a known symbol' do
    get asset_tooltip_path(symbol: 'BTC')

    assert_response :ok
    assert_includes response.body, 'Bitcoin'
    assert_includes response.body, 'BTC'
    assert_includes response.body, 'Crypto' # type label
  end

  test 'symbol match is case-insensitive' do
    get asset_tooltip_path(symbol: 'btc')

    assert_response :ok
    assert_includes response.body, 'Bitcoin'
  end

  test 'when several assets share a symbol it picks the highest market cap (lowest rank)' do
    create(:asset, symbol: 'SUN', name: 'Low Cap Sun', market_cap_rank: nil)
    create(:asset, symbol: 'SUN', name: 'Big Sun', market_cap_rank: 5)

    get asset_tooltip_path(symbol: 'SUN')

    assert_response :ok
    assert_includes response.body, 'Big Sun'
    refute_includes response.body, 'Low Cap Sun'
  end

  test 'renders the card for a non-crypto asset' do
    create(:asset, :usd)

    get asset_tooltip_path(symbol: 'USD')

    assert_response :ok
    assert_includes response.body, 'US Dollar'
  end

  test 'returns 404 for an unknown symbol' do
    get asset_tooltip_path(symbol: 'NOPE')

    assert_response :not_found
  end

  # asset_id disambiguates symbol collisions. A pill that renders a KNOWN asset carries both its
  # symbol and its id, so the card resolves to that exact asset — not the highest-market-cap match
  # for the symbol. (Real case: the stock XYZ / Block Inc on an Alpaca bot vs the crypto "Xyzverse".)
  test 'asset_id takes precedence over a higher-ranked symbol collision' do
    create(:asset, symbol: 'XYZ', name: 'Xyzverse', category: 'Cryptocurrency', market_cap_rank: 50)
    stock = create(:asset, symbol: 'XYZ', name: 'Block Inc', category: 'Stock', market_cap_rank: nil)

    # Both params present — exactly what a .ticker pill sends.
    get asset_tooltip_path(asset_id: stock.id, symbol: 'XYZ')

    assert_response :ok
    assert_includes response.body, 'Block Inc'
    assert_includes response.body, 'Stock' # type label
    refute_includes response.body, 'Xyzverse'
  end

  # A present-but-unknown numeric id must 404, NOT fall back to the symbol guess.
  test 'returns 404 for a nonexistent asset_id even when symbol is present' do
    create(:asset, symbol: 'XYZ', name: 'Xyzverse', category: 'Cryptocurrency', market_cap_rank: 50)

    get asset_tooltip_path(asset_id: 999_999, symbol: 'XYZ')

    assert_response :not_found
    refute_includes response.body, 'Xyzverse'
  end

  # A present-but-invalid asset_id must NOT silently fall back to a symbol guess — that would
  # re-introduce the wrong-asset bug for any pill that fails to resolve by id.
  test 'returns 404 for a non-numeric asset_id even when symbol is present' do
    create(:asset, symbol: 'XYZ', name: 'Xyzverse', category: 'Cryptocurrency', market_cap_rank: 50)

    get asset_tooltip_path(asset_id: 'abc', symbol: 'XYZ')

    assert_response :not_found
    refute_includes response.body, 'Xyzverse'
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
