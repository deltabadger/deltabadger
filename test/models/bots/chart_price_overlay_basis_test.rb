# frozen_string_literal: true

require 'test_helper'

# The chart carries two readings of one market, and a split is the only place they can disagree.
#
# VALUES are as-traded: the metrics walk counts the shares the bot actually bought, so they can
# only be multiplied by the prices the market actually quoted. That pairing is not a preference —
# a bot that bought before a split and sold out before its effective date is never restated by the
# broker, so no ledger row exists and no factor can be recovered. Nothing may convert those counts.
#
# The OVERLAY multiplies nothing. The browser rebases each price line against its own first point
# in view, so it reads as a return, and the return of a raw series across a split is a 90% crash
# that never happened. It gets the venue's history as the venue reads it today.
class ChartPriceOverlayBasisTest < ActiveSupport::TestCase
  SPLIT_AT = Time.utc(2026, 6, 12, 13, 30)
  T0 = SPLIT_AT - 8.days
  NOW = SPLIT_AT + 4.days

  setup do
    travel_to NOW
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @user = create(:user)
    @exchange = create(:alpaca_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
    @usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    @klac = create(:asset, external_id: 'klac', symbol: 'KLAC', category: 'Common Stock')
  end

  teardown { travel_back }

  # As-traded: 1000 a share until the split, 100 after it.
  def raw_candles
    days.map { |day| [day, (day < SPLIT_AT ? 1000 : 100).to_d, 0, 0, 0, 1.to_d] }
  end

  # Restated: the same market, read on today's basis. 100 throughout — the pre-split bars are the
  # 1000 the market quoted, divided by the ten shares each of them became.
  def restated_candles
    days.map { |day| [day, 100.to_d, 0, 0, 0, 1.to_d] }
  end

  def days
    (-9..4).map { |d| SPLIT_AT + d.days }
  end

  def split!
    create(:account_transaction, user: @user, api_key: @api_key, exchange: @exchange,
                                 entry_type: :adjustment, base_currency: 'KLAC', base_amount: 18,
                                 quote_currency: nil, quote_amount: nil, transacted_at: SPLIT_AT,
                                 raw_data: { 'corporate_action' => 'split', 'split_ratio' => '10:1' })
  end

  # Two shares at 1000, held from before the split all the way through it.
  def buy!(bot)
    create(:transaction, bot: bot, exchange: @exchange, base: 'KLAC', quote: 'USD',
                         side: :buy, amount: 2, amount_exec: 2, price: 1000,
                         quote_amount: 2000, quote_amount_exec: 2000, created_at: T0)
  end

  def stub_live(price = 100.to_d)
    Exchanges::Alpaca.any_instance.stubs(:get_last_price).returns(Result::Success.new(price))
    Exchanges::Alpaca.any_instance.stubs(:get_tickers_prices)
                     .returns(Result::Success.new('KLACUSD' => price))
  end

  # Both series answer from the same seam, told apart by `restated:` — the one thing this change
  # adds to the fetch path.
  def stub_candles(bot, raw: raw_candles, restated: restated_candles)
    bot.define_singleton_method(:fetch_candle_series) do |ticker:, since:, timeframe:, restated: false| # rubocop:disable Lint/UnusedBlockArgument
      Result::Success.new(restated ? restated_series : raw_series)
    end
    bot.define_singleton_method(:raw_series) { raw }
    bot.define_singleton_method(:restated_series) { restated }
    bot
  end

  def single_asset_bot
    bot = create(:dca_single_asset, user: @user, exchange: @exchange, with_api_key: false,
                                    base_asset: @klac, quote_asset: @usd)
    buy!(bot)
    bot
  end

  # A basket holding a stock beside a coin — the case the overlay grids have to merge per symbol
  # rather than wholesale. Only KLAC is ever bought, so only KLAC is ever priced.
  def composition_bot
    btc = create(:asset, external_id: 'btc', symbol: 'BTC', category: 'Cryptocurrency')
    bot = create(:dca_multi_asset, user: @user, exchange: @exchange, with_api_key: false,
                                   base_assets: [@klac, btc], quote_asset: @usd)
    buy!(bot)
    bot
  end

  def chart_for(bot)
    bot.metrics_with_current_prices_and_candles(force: true)[:chart]
  end

  # == the two rulers ==

  test 'a single-asset bot values as-traded and draws the overlay restated' do
    split!
    stub_live
    data = chart_for(stub_candles(single_asset_bot))

    # Two shares at 1000, then twenty at 100: 2000 throughout, on either side of the split.
    data[:series][0].each { |value| assert_in_delta 2000.to_d, value.to_d, 1.to_d }
    # And one basis in the line the reader sees — no step, in either direction.
    assert_equal [100.to_d], data[:prices]['KLAC'].compact.uniq
  end

  test 'a multi-asset bot values as-traded and draws the overlay restated' do
    split!
    stub_live
    data = chart_for(stub_candles(composition_bot))

    data[:series][0].each { |value| assert_in_delta 2000.to_d, value.to_d, 1.to_d }
    assert_equal [100.to_d], data[:prices]['KLAC'].compact.uniq
  end

  test 'without the overlay grid the price line is the raw series it always was' do
    # The guard on the whole change: a venue that does not restate history gets one grid, one
    # fetch, and byte-for-byte what it got before.
    @klac.update!(category: 'Cryptocurrency')
    stub_live
    data = chart_for(stub_candles(single_asset_bot))

    assert_equal [1000.to_d, 100.to_d], data[:prices]['KLAC'].compact.uniq
  end

  test 'a crypto ticker is never asked for a second series' do
    @klac.update!(category: 'Cryptocurrency')
    stub_live
    bot = single_asset_bot
    calls = []
    bars = [[SPLIT_AT, 100.to_d, 0, 0, 0, 1.to_d]]
    bot.define_singleton_method(:fetch_candle_series) do |ticker:, since:, timeframe:, restated: false| # rubocop:disable Lint/UnusedBlockArgument
      calls << restated
      Result::Success.new(bars)
    end

    bot.metrics_with_current_prices_and_candles(force: true)

    assert_equal [false], calls.uniq, 'a venue with no corporate actions has one series, not two'
  end

  # == the fallback ==

  test 'a symbol whose restated fetch fails keeps the price line it has' do
    split!
    stub_live
    bot = single_asset_bot
    raw = raw_candles
    bot.define_singleton_method(:fetch_candle_series) do |ticker:, since:, timeframe:, restated: false| # rubocop:disable Lint/UnusedBlockArgument
      restated ? Result::Failure.new('boom') : Result::Success.new(raw)
    end

    data = chart_for(bot)

    # Degraded to today's behaviour — a raw line with the step in it — rather than no line at all.
    assert_equal [1000.to_d, 100.to_d], data[:prices]['KLAC'].compact.uniq
    data[:series][0].each { |value| assert_in_delta 2000.to_d, value.to_d, 1.to_d }
  end

  test 'the value series never reads the restated grid, even where the two disagree' do
    split!
    stub_live
    # A restated series at a WRONG basis would move the values if anything valued against it.
    data = chart_for(stub_candles(single_asset_bot,
                                  restated: days.map { |d| [d, 7.to_d, 0, 0, 0, 1.to_d] }))

    data[:series][0].each { |value| assert_in_delta 2000.to_d, value.to_d, 1.to_d }
    assert_equal [7.to_d], data[:prices]['KLAC'].compact.uniq
  end
end
