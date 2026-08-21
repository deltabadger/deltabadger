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

  test 'single, dual and index render the same columns' do
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    binance = create(:binance_exchange)
    bots = [create(:dca_single_asset, user: @user, exchange: binance, base_asset: btc, quote_asset: usd),
            create(:dca_dual_asset, user: @user, exchange: binance, base0_asset: btc, base1_asset: eth, quote_asset: usd),
            create(:dca_index, user: @user, quote_asset: usd)]

    headers = bots.map do |bot|
      warm_index_prices(bot) if bot.is_a?(Bots::DcaIndex)

      get bot_path(id: bot.id)

      assert_response :success
      css_select('#assets_metrics_table thead th').map { |th| th.text.strip }
    end

    assert_equal 1, headers.uniq.size, "columns drifted: #{headers.inspect}"
    assert_equal ['', '', 'Amount', 'Avg. Price', 'Value', 'P/L'], headers.first
  end

  test 'a bot with nothing invested still shows the table, with a placeholder row' do
    bot = create(:dca_single_asset, user: @user)

    get bot_path(id: bot.id)

    assert_select '#assets_metrics_table tbody tr', 1
    assert_select '#assets_metrics_table tbody td[colspan]', /\./
  end

  private

  def warm_index_prices(bot)
    data = bot.metrics.deep_dup
    data[:asset_values] = { 'AAA' => { amount: 1, quote_invested: 100, current_value: 110,
                                       current_price: 110, avg_price: 100, pnl_percentage: 0.1 } }
    Rails.cache.write(bot.send(:metrics_with_current_prices_cache_key), data)
  end
end
