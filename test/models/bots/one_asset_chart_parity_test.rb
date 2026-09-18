require 'test_helper'

# A one-asset multi-asset bot charts what the single-asset bot it replaces charts, over the same fills,
# candles and live price — including when the candles start late, are missing, or the live read fails.
# (A sale that never reported its proceeds is booked differently by the two; the conversion refuses
# those histories — Bot::SingleToComposition.)
class Bots::OneAssetChartParityTest < ActiveSupport::TestCase
  T0 = Time.zone.parse('2026-06-01 12:00')

  setup do
    travel_to T0 + 6.days
    @btc = create(:asset, :bitcoin)
    @usd = create(:asset, :usd)
    user = create(:user)
    @pair = create(:dca_single_asset, user:, base_asset: @btc, quote_asset: @usd)
    @basket = create(:dca_multi_asset, user:, exchange: @pair.exchange, base_assets: [@btc], quote_asset: @usd)
    @ticker = @pair.ticker
    [@pair, @basket].each do |bot|
      buy(bot, at: T0 + 1.day + 5.hours, price: 100) # between daily candles, as fills are
      buy(bot, at: T0 + 3.days + 7.hours, price: 90)
    end
    live(210)
  end

  teardown { travel_back }

  test 'with candles over the whole history' do
    candles(from: T0, price: 200)

    assert_same_chart
  end

  test 'with candles that start after the first purchase' do
    candles(from: T0 + 2.days, price: 200)

    assert_same_chart
  end

  test 'with no candles at all' do
    candles(from: nil)

    assert_same_chart
  end

  test 'with an order resting at another price before the candles start' do
    candles(from: T0 + 2.days, price: 200)
    [@pair, @basket].each { |bot| buy(bot, at: T0 + 1.day + 8.hours, price: 50, status: :open) }

    assert_same_chart
  end

  test 'with two fills at one moment before the candles start' do
    candles(from: T0 + 2.days, price: 200)
    [@pair, @basket].each do |bot|
      buy(bot, at: T0 + 1.day + 9.hours, price: 100)
      buy(bot, at: T0 + 1.day + 9.hours, price: 80)
    end

    assert_same_chart
  end

  test 'with the live price unavailable' do
    candles(from: T0 + 2.days, price: 200)
    Exchanges::Binance.any_instance.stubs(:get_tickers_prices).returns(Result::Failure.new('down'))

    assert @pair.metrics_with_current_prices(force: true)[:prices_stale]
    assert @basket.metrics_with_current_prices(force: true)[:prices_stale]
    assert_same_chart
  end

  private

  def assert_same_chart
    pair = @pair.metrics_with_current_prices_and_candles(force: true)[:chart]
    basket = @basket.metrics_with_current_prices_and_candles(force: true)[:chart]

    assert_equal pair[:labels].map(&:to_i), basket[:labels].map(&:to_i)
    assert_equal(pair[:series][0].map { |value| rounded(value) }, basket[:series][0].map { |value| rounded(value) })
  end

  def rounded(value) = value.to_d.round(8)

  def buy(bot, at:, price:, status: :closed, side: :buy)
    filled = status == :closed
    create(:transaction, bot:, exchange: bot.exchange, status: :submitted, external_status: status, side:,
                         external_id: "b-#{bot.id}-#{SecureRandom.hex(3)}", base: 'BTC', quote: 'USD', price:, amount: 1,
                         amount_exec: (1 if filled), quote_amount: price, quote_amount_exec: (price if filled), created_at: at)
  end

  # Daily opens from `from` to now, served to both bots the same way.
  def candles(from:, price: nil)
    series = from.nil? ? [] : (0..((Time.current - from) / 1.day).floor).map { |day| [from + day.days, price.to_d] }
    [@pair, @basket].each do |bot|
      bot.define_singleton_method(:fetch_candle_series) { |**| Result::Success.new(series) }
    end
  end

  def live(price)
    Exchanges::Binance.any_instance.stubs(:get_tickers_prices)
                      .returns(Result::Success.new(@ticker.ticker => price.to_d))
  end
end
