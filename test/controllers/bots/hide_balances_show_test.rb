# frozen_string_literal: true

require 'test_helper'

# A bot's own page with balances hidden: the chart reads in Return only, the invested/value panel
# is gone, and the holdings table keeps the symbol, its average price and its P/L but states
# neither how much is held nor what it is worth.
class Bots::HideBalancesShowTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true) # satisfies the onboarding gate
    @user = create(:user, hide_balances: true)
    @quote = create(:asset, :eur)
    @bot = create(:dca_index, user: @user, quote_asset: @quote)
    # The chart widget only renders for a bot that has traded.
    create(:ticker, exchange: @bot.exchange, base_asset: create(:asset, symbol: 'AAA'), quote_asset: @quote)
    create(:transaction, bot: @bot, base: 'AAA', quote: 'EUR')
    sign_in @user

    @store = ActiveSupport::Cache::MemoryStore.new
    Rails.stubs(:cache).returns(@store)
  end

  def seed_chart
    data = @bot.metrics.deep_dup
    data[:chart][:labels] = [Time.utc(2026, 1, 1), Time.utc(2026, 2, 1)]
    data[:chart][:series] = [[100.0, 260.0], [100.0, 200.0]]
    Rails.cache.write(@bot.send(:metrics_with_current_prices_and_candles_cache_key), data)
  end

  # --- the chart --------------------------------------------------------------------------

  test 'the chart offers no Value mode to switch to' do
    seed_chart

    get bot_path(id: @bot.id)

    assert_select '.widget--chart__modes .segmented__option', false
  end

  test 'the chart comes up in Return mode rather than Value' do
    seed_chart

    get bot_path(id: @bot.id)

    assert_select '#chart [data-bot--chart-pnl-only-value="true"]', 1
  end

  test 'the chart summary keeps its percentage and drops its money' do
    seed_chart

    get bot_path(id: @bot.id)

    assert_select '[data-bot--chart-target="percent"]', 1
    assert_select '[data-bot--chart-target="pnl"]', false
  end

  # --- the metrics panel ------------------------------------------------------------------

  test 'the invested and portfolio-value panel is not there' do
    get bot_path(id: @bot.id)

    assert_select '.widget.data-grid', false
  end

  test 'a single-asset bot has no panel either' do
    bot = create(:dca_single_asset, user: @user, quote_asset: @quote)

    get bot_path(id: bot.id)

    assert_select '.widget.data-grid', false
  end

  test 'a dual-asset bot has no panel either' do
    bot = create(:dca_dual_asset, user: @user, quote_asset: @quote)

    get bot_path(id: bot.id)

    assert_select '.widget.data-grid', false
  end

  # --- the holdings table -----------------------------------------------------------------

  # A single-asset bot always renders the holdings table, empty rows included — the composition
  # one needs live asset values before it draws anything.
  test 'the holdings table drops Amount and Value and keeps the rest' do
    bot = create(:dca_single_asset, user: @user, quote_asset: @quote)

    get bot_path(id: bot.id)

    headers = css_select('#assets_metrics_table thead th').map { |th| th.text.strip }
    assert_not_includes headers, I18n.t('data_labels.amount')
    assert_not_includes headers, I18n.t('data_labels.value')
    assert_includes headers, I18n.t('data_labels.avg_price')
    assert_includes headers, I18n.t('data_labels.pnl')
  end

  test 'the holdings table keeps both columns when balances are shown' do
    @user.update!(hide_balances: false)
    bot = create(:dca_single_asset, user: @user, quote_asset: @quote)

    get bot_path(id: bot.id)

    headers = css_select('#assets_metrics_table thead th').map { |th| th.text.strip }
    assert_includes headers, I18n.t('data_labels.amount')
    assert_includes headers, I18n.t('data_labels.value')
  end

  # --- the log ----------------------------------------------------------------------------

  test 'the log drops Amount and Value and keeps Date and Price' do
    get bot_path(id: @bot.id)

    headers = css_select('#orders_list').first.parent.parent.css('thead th').map { |th| th.text.strip }
    assert_not_includes headers, I18n.t('data_labels.amount')
    assert_not_includes headers, I18n.t('data_labels.value')
    assert_includes headers, I18n.t('data_labels.date')
    assert_includes headers, I18n.t('data_labels.price')
  end
end
