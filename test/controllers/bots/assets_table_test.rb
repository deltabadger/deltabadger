# frozen_string_literal: true

require 'test_helper'

# Every bot type shows the same holdings table. They used to be three hand-copied tables that drifted
# apart — different headers, different translation keys, a bold P/L on two of them and a plain one on
# the third. This pins them to one partial.
class Bots::AssetsTableTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    sign_in @user

    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
  end

  test 'single, index and multi render the same columns' do
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    binance = create(:binance_exchange)
    bots = [create(:dca_single_asset, user: @user, exchange: binance, base_asset: btc, quote_asset: usd),
            create(:dca_index, user: @user, quote_asset: usd),
            create(:dca_multi_asset, user: @user, exchange: binance, base_assets: [btc, eth], quote_asset: usd)]

    headers = bots.map do |bot|
      warm_composition_prices(bot) if bot.respond_to?(:composition_tickers)

      get bot_path(id: bot.id)

      assert_response :success
      css_select('#assets_metrics_table thead th').map { |th| th.text.strip }
    end

    assert_equal 1, headers.uniq.size, "columns drifted: #{headers.inspect}"
    assert_equal ['', '', 'Amount', 'Avg. Price', 'Value', 'P/L'], headers.first
  end

  # The chart hovers against these, one holding at a time; without the symbol on the row there is
  # nothing for it to key on.
  test 'every holding row carries the symbol the chart draws it by' do
    usd = create(:asset, :usd)
    bot = create(:dca_index, user: @user, quote_asset: usd)
    warm_composition_prices(bot)

    get bot_path(id: bot.id)

    assert_select '#assets_metrics_table tbody tr[data-symbol="AAA"]', 1
  end

  # A bot's money is in the currency it trades in, and a code is how a transaction currency is
  # written — the symbol is reserved for the reader's own denomination. The chart's headline
  # already says USDT; the tiles and the table under it said nothing.
  test 'invested, value and prices carry the quote currency code beside the figure' do
    btc = create(:asset, :bitcoin)
    usd = create(:asset, :usd)
    binance = create(:binance_exchange)
    index = create(:dca_index, user: @user, quote_asset: usd)
    warm_composition_prices(index)

    get bot_path(id: index.id)

    tiles = css_select('#metrics .data-grid__item__value').map { |node| node.text.gsub(/\s+/, ' ').strip }
    assert_equal 2, tiles.size
    tiles.each { |tile| assert_match(/\A[\d,.]+ USD\z/, tile) }
    cells = css_select('#assets_metrics_table tbody tr[data-symbol="AAA"] td').map { |node| node.text.gsub(/\s+/, ' ').strip }
    assert_equal '100.00 USD', cells[3], 'average price'
    assert_equal '110.00 USD', cells[4], 'value'
    assert_select '#assets_metrics_table tbody tr[data-symbol="AAA"] td small', text: 'USD', count: 2

    single = create(:dca_single_asset, user: @user, exchange: binance, base_asset: btc, quote_asset: usd)

    get bot_path(id: single.id)

    # Portfolio value is a spinner on a first load; invested is not.
    tiles = css_select('#metrics .data-grid__item__value').reject { |tile| tile.css('.loader--small').any? }
    assert_equal 1, tiles.size
    assert_equal '0 USD', tiles.first.text.gsub(/\s+/, ' ').strip
  end

  test 'a bot with nothing invested still shows the table, with a placeholder row' do
    bot = create(:dca_single_asset, user: @user)

    get bot_path(id: bot.id)

    assert_select '#assets_metrics_table tbody tr', 1
    assert_select '#assets_metrics_table tbody td[colspan]', /\./
  end

  private

  def warm_composition_prices(bot)
    data = bot.metrics.deep_dup
    symbol = bot.dca_multi_asset? ? bot.base_assets.first.symbol : 'AAA'
    data[:asset_values] = { symbol => { amount: 1, quote_invested: 100, current_value: 110,
                                        current_price: 110, avg_price: 100, pnl_percentage: 0.1 } }
    Rails.cache.write(bot.send(:metrics_with_current_prices_cache_key), data)
  end
end
