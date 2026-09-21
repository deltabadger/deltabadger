require 'test_helper'
require Rails.root.join('db/migrate/20260921120000_repoint_gemini_perp_tickers_to_spot.rb')

# Gemini lists perpetuals under the same base and quote as spot, and the catalogue sync matched
# rows on base and quote, so some rows ended up pointing at a perpetual (btcgusdperp) instead of
# the spot pair (btcgusd). The migration moves each such row back to its spot twin from a frozen
# snapshot, and takes a row out of trading where there is no twin to move to.
class RepointGeminiPerpTickersToSpotTest < ActiveSupport::TestCase
  setup do
    @gemini = create(:gemini_exchange)
    @gusd = create(:asset, symbol: 'GUSD', name: 'Gemini Dollar', external_id: 'gemini-dollar')
  end

  test 'a perpetual row takes its spot twin: symbol, minimum and precisions' do
    ticker = gemini_ticker('BTC', 'btcgusdperp', available: false, trading_enabled: false)

    migrate!

    ticker.reload
    assert_equal 'btcgusd', ticker.ticker
    assert_equal '0.00001'.to_d, ticker.minimum_base_size
    assert_equal [8, 2, 2], [ticker.base_decimals, ticker.quote_decimals, ticker.price_decimals]
    assert ticker.trading_enabled, 'trading_enabled comes from the snapshot'
    refute ticker.available, 'availability is left for the next sync to decide'
  end

  test 'a perpetual with no spot twin is taken out of trading' do
    ticker = gemini_ticker('MEW', 'mewgusdperp')

    migrate!

    ticker.reload
    assert_equal 'mewgusdperp', ticker.ticker
    refute ticker.available
    refute ticker.trading_enabled
  end

  test 'a twin whose symbol another row already holds is not taken; the other row is untouched' do
    perp = gemini_ticker('ETH', 'ethgusdperp')
    holder = gemini_ticker('WETH', 'ethgusd')
    before = holder.attributes

    migrate!

    perp.reload
    assert_equal 'ethgusdperp', perp.ticker
    refute perp.available
    refute perp.trading_enabled
    assert_equal before, holder.reload.attributes
  end

  test 'a second run changes nothing' do
    gemini_ticker('BTC', 'btcgusdperp')
    gemini_ticker('MEW', 'mewgusdperp')
    gemini_ticker('ETH', 'ethgusdperp')
    gemini_ticker('WETH', 'ethgusd')
    migrate!
    # Backdated, so a rerun that wrote anything at all would show: the migration stamps updated_at
    # from SQLite's clock, which has one-second resolution.
    Ticker.update_all(updated_at: 1.day.ago)
    after_first = Ticker.order(:id).map(&:attributes)

    migrate!

    assert_equal after_first, Ticker.order(:id).map(&:attributes)
  end

  test 'another exchange is untouched, whatever its symbols end in' do
    ticker = create(:ticker, exchange: create(:binance_exchange), ticker: 'btcgusdperp')
    before = ticker.attributes

    migrate!

    assert_equal before, ticker.reload.attributes
  end

  # Bots and transactions name a pair by its asset ids (and base/quote), bot_index_assets by the
  # ticker row's id, and an order is tracked by its order id — so a referenced row is repointed in
  # place like any other, and every reference still reaches it.
  test 'a row an index bot holds is repointed, and the bot still holds it' do
    ticker = gemini_ticker('BTC', 'btcgusdperp')
    bot = create(:dca_single_asset, exchange: create(:binance_exchange))
    now = Time.current.to_fs(:db)
    ActiveRecord::Base.connection.execute(<<~SQL.squish)
      INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, created_at, updated_at)
      VALUES (#{bot.id}, #{ticker.base_asset_id}, #{ticker.id}, '#{now}', '#{now}')
    SQL

    migrate!

    assert_equal 'btcgusd', ticker.reload.ticker
    ticker_id = ActiveRecord::Base.connection.select_value("SELECT ticker_id FROM bot_index_assets WHERE bot_id = #{bot.id}")
    assert_equal ticker.id, ticker_id
  end

  test 'an install without Gemini is left alone' do
    @gemini.destroy!

    assert_nothing_raised { migrate! }
  end

  test 'the migration is irreversible' do
    assert_raises(ActiveRecord::IrreversibleMigration) { RepointGeminiPerpTickersToSpot.new.down }
  end

  private

  def gemini_ticker(base, symbol, **attrs)
    create(:ticker, exchange: @gemini, base_asset: create(:asset, symbol: base), quote_asset: @gusd,
                    base: base, quote: 'GUSD', ticker: symbol,
                    minimum_base_size: 0.1, base_decimals: 1, quote_decimals: 1, price_decimals: 1, **attrs)
  end

  def migrate!
    RepointGeminiPerpTickersToSpot.new.tap { |m| m.verbose = false }.up
  end
end
