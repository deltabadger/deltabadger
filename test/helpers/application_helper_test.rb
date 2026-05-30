require 'test_helper'

class ApplicationHelperTest < ActionView::TestCase
  # ticker_class_for is the value-based core: it gets the RAW persisted color (nullable),
  # NOT a fallback, so it can tell "real stock color exists" from "no color → distinct fallback".

  test 'ticker_class_for: a stock with no color uses the distinct fallback styling' do
    assert_equal 'ticker ticker--stock', ticker_class_for(category: 'Stock', color: nil)
    assert_equal 'ticker ticker--stock', ticker_class_for(category: 'Stock', color: '')
  end

  test 'ticker_class_for: a stock WITH a real color renders as a normal colored ticker' do
    assert_equal 'ticker', ticker_class_for(category: 'Stock', color: '#4285F4')
  end

  test 'ticker_class_for: non-stock assets are always a plain ticker' do
    assert_equal 'ticker', ticker_class_for(category: 'Cryptocurrency', color: nil)
    assert_equal 'ticker', ticker_class_for(category: 'Cryptocurrency', color: '#F7931A')
  end

  test 'ticker_class(asset) delegates to ticker_class_for using the asset category + color' do
    colorless_stock = build(:asset, category: 'Stock', color: nil)
    assert_equal 'ticker ticker--stock', ticker_class(colorless_stock)

    colored_stock = build(:asset, category: 'Stock', color: '#4285F4')
    assert_equal 'ticker', ticker_class(colored_stock)

    crypto = build(:asset, category: 'Cryptocurrency', color: nil)
    assert_equal 'ticker', ticker_class(crypto)
  end
end
