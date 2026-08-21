# frozen_string_literal: true

require 'test_helper'

# The index page splits its holdings in two: what the index currently wants, and what has left it.
# Before this they were one undifferentiated table, so a coin the index dropped was indistinguishable
# from a current constituent.
class Bots::DcaIndexes::QuittersTableTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    @bot = create(:dca_index, user: @user)
    sign_in @user

    @store = ActiveSupport::Cache::MemoryStore.new
    Rails.stubs(:cache).returns(@store)

    @assets = %w[AAA CCC].to_h do |symbol|
      asset = create(:asset, symbol: symbol, name: "Coin #{symbol}", external_id: "coin-#{symbol.downcase}")
      ticker = create(:ticker, exchange: @bot.exchange, base_asset: asset, quote_asset: @bot.quote_asset)
      [symbol, { asset: asset, ticker: ticker }]
    end
  end

  test 'a quitter is listed under its own heading, not among the index members' do
    in_index('AAA')
    exited('CCC')
    warm_prices({ 'AAA' => 100, 'CCC' => 20 })

    get bot_path(id: @bot.id)

    assert_response :success
    assert_select '#assets_metrics_table tbody tr', 1
    assert_select '#exited_metrics_table tbody tr', 1
    assert_select '#exited_metrics_table', /CCC/
  end

  test 'the quitters section offers the sell, through the confirmation modal' do
    # Not a bare form with a browser confirm(): a market sale of every quitter is the most
    # destructive action on the page and gets the app's own modal, which lists what it will sell.
    in_index('AAA')
    exited('CCC')
    warm_prices({ 'AAA' => 100, 'CCC' => 20 })

    get bot_path(id: @bot.id)

    assert_select "#exited_metrics_table a[href='#{new_bot_liquidation_path(bot_id: @bot.id)}'][data-turbo-frame='modal']", 1
    assert_select "#exited_metrics_table form[action='#{bot_liquidation_path(bot_id: @bot.id)}']", 0
  end

  test 'no quitters means no section at all' do
    in_index('AAA', 'CCC')
    warm_prices({ 'AAA' => 100, 'CCC' => 20 })

    get bot_path(id: @bot.id)

    assert_select '#exited_metrics_table', 0
    assert_select '#assets_metrics_table tbody tr', 2
  end

  test 'a halted liquidation replaces the sell with the clear' do
    # Re-running is the one thing that must not be offered: the last placement's outcome is unknown.
    in_index('AAA')
    exited('CCC')
    warm_prices({ 'AAA' => 100, 'CCC' => 20 })
    @bot.start_liquidation_placement!('CCC')
    @bot.flag_liquidation_ambiguous!

    get bot_path(id: @bot.id)

    assert_select "#exited_metrics_table form[action='#{bot_liquidation_path(bot_id: @bot.id)}']", 0
    assert_select '#exited_metrics_table form[action*="liquidation_resolutions"]', 1
  end

  test 'realised P/L only appears once something has been realised' do
    in_index('AAA')
    warm_prices({ 'AAA' => 100 }, realised: 0)

    get bot_path(id: @bot.id)

    assert_select '.data-grid__item', 2
  end

  test 'a realised loss is shown as a negative figure' do
    in_index('AAA')
    warm_prices({ 'AAA' => 100 }, realised: -40)

    get bot_path(id: @bot.id)

    assert_select '.data-grid__item', 3
    assert_select '.data-grid__item .text-danger', /40/
  end

  test 'the clear survives a failed price read' do
    # The network trouble that empties asset_values is the same trouble that produces an unresolved
    # placement — so nesting the halt under live prices hides it exactly when it is needed.
    in_index('AAA')
    exited('CCC')
    warm_prices({})
    @bot.start_liquidation_placement!('CCC')
    @bot.flag_liquidation_ambiguous!

    get bot_path(id: @bot.id)

    assert_response :success
    assert_select '#exited_metrics_table form[action*="liquidation_resolutions"]', 1
  end

  private

  def in_index(*symbols)
    symbols.each do |symbol|
      BotIndexAsset.create!(bot: @bot, asset: @assets[symbol][:asset], ticker: @assets[symbol][:ticker],
                            target_allocation: 1.0 / symbols.size, in_index: true, entered_at: Time.current)
    end
  end

  def exited(symbol)
    BotIndexAsset.create!(bot: @bot, asset: @assets[symbol][:asset], ticker: @assets[symbol][:ticker],
                          target_allocation: nil, in_index: false, exited_at: Time.current)
  end

  def warm_prices(values, realised: 0)
    data = @bot.metrics.deep_dup
    data[:realised_pnl] = realised.to_d
    data[:asset_values] = values.transform_values do |v|
      { amount: v.to_d / 100, quote_invested: v.to_d, current_value: v.to_d,
        current_price: 100, avg_price: 100, pnl_percentage: 0 }
    end
    Rails.cache.write(@bot.send(:metrics_with_current_prices_cache_key), data)
  end
end
