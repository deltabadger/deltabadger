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

  # --- asset_type_label: the tooltip hover-card's info line --------------------
  # Maps the asset category to a friendly type label; returns nil for unknown/blank so
  # the info line is omitted rather than mislabeled (single swappable field for a future
  # real description).

  test 'asset_type_label maps known categories to friendly labels' do
    assert_equal 'Crypto', asset_type_label('Cryptocurrency')
    assert_equal 'Stock', asset_type_label('Stock')
    assert_equal 'Stock', asset_type_label('Common Stock')
    assert_equal 'ETF', asset_type_label('ETF')
    assert_equal 'Fund', asset_type_label('Fund')
    assert_equal 'Cash', asset_type_label('Fiat')
    assert_equal 'Cash', asset_type_label('Currency')
  end

  test 'asset_type_label returns nil for unknown or blank categories' do
    assert_nil asset_type_label('Something Weird')
    assert_nil asset_type_label(nil)
    assert_nil asset_type_label('')
  end

  # Table dates are year-first in every locale; only the clock changes shape with the language.
  test 'table_when: English shows a 12-hour clock, other locales a 24-hour one' do
    at = Time.utc(2026, 8, 21, 13, 19)

    I18n.with_locale(:en) do
      assert_equal '2026/08/21 <small>3:19 pm</small>', table_when(at, 'Europe/Warsaw')
      assert_equal '2026/08/21 <small>9:19 am</small>', table_when(at, 'America/New_York')
    end

    I18n.with_locale(:pl) do
      assert_equal '2026/08/21 <small>15:19</small>', table_when(at, 'Europe/Warsaw')
    end
  end

  test 'every locale carries its own table clock format' do
    I18n.available_locales.each do |locale|
      assert I18n.exists?('time.formats.table_clock', locale, fallback: false), "#{locale} falls back for the table clock"
    end
  end

  # --- asset_logo_url: cash draws its flag, everything else its own image -----
  # The flag wins over whatever the row carries, so a hosted install (which now receives a fiat
  # logo_url from the market-data API) and a self-hosted one (which never will) draw the same
  # currency the same way.

  test 'asset_logo_url: a currency draws its flag, over any image it carries' do
    assert_equal image_path('flags/us.svg'), asset_logo_url(nil, symbol: 'USD', category: 'Currency')
    assert_equal image_path('flags/eu.svg'), asset_logo_url(nil, symbol: 'EUR', category: 'Currency')
    assert_equal image_path('flags/us.svg'),
                 asset_logo_url('https://data.example.com/logos/26/101.svg', symbol: 'USD', category: 'Fiat')
  end

  test 'asset_logo_url: everything else keeps its own image' do
    coingecko = 'https://coin-images.coingecko.com/coins/images/1/large/bitcoin.png'
    assert_equal coingecko, asset_logo_url(coingecko, symbol: 'BTC', category: 'Cryptocurrency')
    assert_nil asset_logo_url(nil, symbol: 'BTC', category: 'Cryptocurrency')
  end

  # `usd` is also a CoinGecko coin id — a micro-cap token — and AccountBalance::Sync already keeps
  # it out of pricing by CATEGORY for the same reason. A token ticker'd USD is not the dollar.
  test 'asset_logo_url: a token that happens to be ticker\'d USD gets no flag' do
    token = 'https://coin-images.coingecko.com/coins/images/9999/large/usd.png'
    assert_equal token, asset_logo_url(token, symbol: 'USD', category: 'Cryptocurrency')
  end

  # The transactions table resolves its rows through Tracker::Ledger.asset_index, which can still
  # come back empty for a symbol nothing on the account holds. A currency is drawable anyway.
  test 'asset_logo_url: a currency with no asset row behind it still draws its flag' do
    assert_equal image_path('flags/gb.svg'), asset_logo_url(nil, symbol: 'GBP', category: nil)
    assert_nil asset_logo_url(nil, symbol: 'BTC', category: nil)
  end
end
