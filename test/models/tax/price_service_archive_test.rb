require 'test_helper'

# Asking for one day's price.
#
# The data API serves its price ARCHIVE only for spans wider than 90 days — at or below that,
# CoinGecko returns sub-daily density the archive cannot reproduce, so it steps aside for the live
# proxy. The proxy's plan then refuses anything older than two years. A single-day question fell
# straight between them: for every date before the CoinGecko window it came back empty, even though
# the archive held the price all along. Verified against production — the same start date returns
# nothing as one day and $56,020 as part of a six-month span.
#
# A missing price is not a cosmetic gap: the lot gets a zero basis, the disposal is stamped
# data_incomplete, and the tracker withholds the P/L on the whole round-trip.
class Tax::PriceServiceArchiveTest < ActiveSupport::TestCase
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    @day = Date.new(2021, 3, 10)
    # Milliseconds at UTC midnight, which is what the archive returns — a local-midnight fixture
    # lands on the day before west of Greenwich and silently tests the neighbour.
    @ms = ->(date) { date.to_time(:utc).to_i * 1000 }
    @service = Tax::PriceService.new
    create(:asset, :bitcoin)
  end

  # The archive's own threshold, so the request has to clear it.
  MIN_ARCHIVE_SPAN = 90

  test 'a single day is asked for as a window the archive will answer' do
    span = nil
    MarketData.stubs(:get_historical_price_range).with do |args|
      span = (args[:to].to_date - args[:from].to_date).to_i
      true
    end.returns(Result::Success.new('prices' => [[@ms.call(@day), 56_020.0]]))

    @service.price_at(asset: 'BTC', currency: 'USD', timestamp: @day.to_time(:utc))

    assert_operator span, :>, MIN_ARCHIVE_SPAN,
                    'at or below 90 days the archive steps aside and the proxy cannot reach back'
  end

  test 'the day asked for is the day returned, not whatever came back first' do
    prices = [
      [@ms.call(@day - 1), 54_000.0],
      [@ms.call(@day), 56_020.0],
      [@ms.call(@day + 1), 57_789.0]
    ]
    MarketData.stubs(:get_historical_price_range).returns(Result::Success.new('prices' => prices))

    assert_equal 56_020.to_d, @service.price_at(asset: 'BTC', currency: 'USD', timestamp: @day.to_time(:utc))
  end

  # The days either side are real prices that were fetched anyway. Storing them is what stops the
  # next row on a neighbouring date from spending another round trip.
  test 'the days fetched on the way past are kept' do
    prices = [
      [@ms.call(@day - 1), 54_000.0],
      [@ms.call(@day), 56_020.0]
    ]
    MarketData.stubs(:get_historical_price_range).returns(Result::Success.new('prices' => prices))

    @service.price_at(asset: 'BTC', currency: 'USD', timestamp: @day.to_time(:utc))

    assert_equal 54_000.to_d, HistoricalPrice.lookup(asset: 'BTC', currency: 'USD', date: @day - 1)
  end

  test 'a window that really has no price still says so rather than inventing one' do
    MarketData.stubs(:get_historical_price_range).returns(Result::Success.new('prices' => []))

    assert_equal 0.to_d, @service.price_at(asset: 'BTC', currency: 'USD', timestamp: @day.to_time(:utc))
    assert_includes @service.warnings, 'BTC/USD 2021-03-10'
  end

  # Tomorrow has no price and never will; a window running past today would ask for it every time.
  test 'the window never runs past today' do
    latest = nil
    MarketData.stubs(:get_historical_price_range).with do |args|
      latest = args[:to].to_date
      true
    end.returns(Result::Success.new('prices' => []))

    @service.price_at(asset: 'BTC', currency: 'USD', timestamp: 2.days.ago)

    assert_operator latest, :<=, Date.current + 1
  end
end
