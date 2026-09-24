# frozen_string_literal: true

require 'test_helper'

# The chart's cached payload grows with the bot's history and its number of assets, and reading it
# is most of what a bot page costs. So the page renders a placeholder and the chart arrives in its
# own lazy frame; the frame is also the one place that asks for a metrics refresh when a cache is cold.
class Bots::ChartFrameTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true) # satisfies the onboarding gate
    @user = create(:user)
    @quote = create(:asset, :eur)
    @bot = create(:dca_index, user: @user, quote_asset: @quote)
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
    @store.write(@bot.send(:metrics_with_current_prices_and_candles_cache_key), data)
    data
  end

  def seed_prices
    @store.write(@bot.send(:metrics_with_current_prices_cache_key), @bot.metrics.deep_dup)
  end

  def frame_src
    src = css_select('turbo-frame#bot_chart').first&.[]('src')
    assert src, 'expected the lazy chart frame on the page'
    src
  end

  test 'the page leaves the chart to a lazy frame and never reads its cache' do
    seed_chart
    seed_prices
    Bots::DcaIndex.any_instance.expects(:metrics_with_current_prices_and_candles_from_cache).never

    get bot_path(id: @bot.id)

    assert_response :success
    assert_equal bot_chart_path(bot_id: @bot.id), frame_src
    assert_select 'turbo-frame#bot_chart #chart .widget--chart__plot .loader', 1
    assert_select '[data-controller="bot--chart"]', 0
    # The refresh request belongs to the frame alone: two askers would enqueue two jobs.
    assert_select '[data-broadcast--on-connect-method-value="metrics_update"]', 0
  end

  test 'the frame renders the chart from a warm cache without asking for a refresh' do
    seed_chart

    get bot_chart_path(bot_id: @bot.id)

    assert_response :success
    assert_select 'turbo-frame#bot_chart #chart [data-controller="bot--chart"]', 1
    assert_select '[data-broadcast--on-connect-method-value="metrics_update"]', 0
  end

  test 'a cold chart cache gets the placeholder and one refresh request' do
    get bot_chart_path(bot_id: @bot.id)

    assert_response :success
    assert_select 'turbo-frame#bot_chart #chart .widget--chart__plot .loader', 1
    assert_select '[data-broadcast--on-connect-method-value="metrics_update"]', 1
  end

  # The page's own panels read the prices cache. When that one was cold, the refresh it needs is
  # asked for through the frame, even though the chart itself had something to draw.
  test 'a page whose prices cache was cold has the frame ask for the refresh' do
    seed_chart

    get bot_path(id: @bot.id)
    assert_equal bot_chart_path(bot_id: @bot.id, metrics_missing: 1), frame_src

    get frame_src
    assert_select '#chart [data-controller="bot--chart"]', 1
    assert_select '[data-broadcast--on-connect-method-value="metrics_update"]', 1
  end

  test 'a warm prices cache leaves the frame URL bare' do
    seed_prices

    get bot_path(id: @bot.id)

    assert_equal bot_chart_path(bot_id: @bot.id), frame_src
  end

  test "another user's bot is not found" do
    other = create(:dca_index, user: create(:user), exchange: @bot.exchange, quote_asset: @quote)

    get bot_chart_path(bot_id: other.id)

    assert_response :not_found
  end
end
