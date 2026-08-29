require 'test_helper'
require Rails.root.join('db/migrate/20260830120000_clear_restated_stock_aths.rb')

# A stock split rewrites every bar of a symbol's history, so an all-time high carried forward
# across one is not a high — it is a number in units that no longer exist.
class ClearRestatedStockAthsTest < ActiveSupport::TestCase
  setup do
    @alpaca = create(:alpaca_exchange)
    @usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
  end

  test 'an Alpaca stock ticker loses its high, and its update stamp with it' do
    ticker = stock_ticker('KLAC', ath: 2411, ath_updated_at: 1.day.ago)

    migrate!

    ticker.reload
    assert_nil ticker.ath
    assert_nil ticker.ath_updated_at
  end

  test 'crypto on the same venue keeps its high' do
    ticker = crypto_ticker('AAVE', exchange: @alpaca, ath: 300)

    migrate!

    assert_equal 300.to_d, ticker.reload.ath
  end

  test 'another venue is untouched' do
    ticker = crypto_ticker('BTC', exchange: create(:binance_exchange), ath: 120_000)

    migrate!

    assert_equal 120_000.to_d, ticker.reload.ath
  end

  test 'a stock ticker that never seeded a high is left as it is' do
    ticker = stock_ticker('AAPL', ath: nil, ath_updated_at: nil)

    migrate!

    assert_nil ticker.reload.ath
  end

  def stock_ticker(symbol, ath:, ath_updated_at:)
    asset = create(:asset, external_id: symbol.downcase, symbol: symbol, category: 'Stock')
    create(:ticker, exchange: @alpaca, base_asset: asset, quote_asset: @usd, ticker: symbol,
                    ath: ath, ath_updated_at: ath_updated_at)
  end

  def crypto_ticker(symbol, exchange:, ath:)
    asset = create(:asset, external_id: symbol.downcase, symbol: symbol, category: 'Cryptocurrency')
    create(:ticker, exchange: exchange, base_asset: asset, quote_asset: @usd,
                    ticker: "#{symbol}/USD", ath: ath, ath_updated_at: 1.day.ago)
  end

  def migrate!
    ClearRestatedStockAths.new.tap { |m| m.verbose = false }.up
  end
end
