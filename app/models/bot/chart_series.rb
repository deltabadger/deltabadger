# frozen_string_literal: true

# One ruler for the bot chart: every point valued at MARKET.
#
# `metrics` can only mark holdings at the price of the transaction that produced them — it has
# no prices of its own — so a transaction point used to re-price the WHOLE position at that one
# fill, while the candle points around it priced the same holdings at market. Two rulers on one
# line: a fill away from market showed as a dip at the exact moment the bot got a good fill, and
# RETURN, which reads every step as performance, compounded the teeth instead of averaging them.
#
# Here the grid is the ruler: the venue's candle opens, plus the live price as a final mark.
#
# A transaction keeps its own point. Time-weighting needs it — `r(t) = (V(t) − flow) / V(t−1)`
# is exact only when the flow lands ON the point being measured, and collapsing a purchase and
# a price move into one step credits the deposit's own growth to performance.
#
# COVERAGE is per symbol: a time is covered when that symbol's grid has a mark at-or-before it
# AND at-or-after it, so interpolation has two real endpoints (an exact hit satisfies both and
# is used directly). A point is re-marked only when every symbol held at that moment is covered
# — otherwise one asset would be summed at market with another at zero. An uncovered
# transaction point keeps the fill mark `metrics` gave it; an uncovered candle point is dropped,
# having no fill mark to fall back to.
module Bot::ChartSeries
  extend ActiveSupport::Concern

  # The seam every candle fetch goes through. Overridable in tests, and the reason the index's
  # bounded-parallel fetch can be exercised without touching the cache.
  def fetch_candle_series(ticker:, since:, timeframe:)
    CandleSeriesCache.fetch(ticker: ticker, since: since, timeframe: timeframe)
  end

  # Candles are fetched from one timeframe BEFORE the first transaction so that the first buy
  # has a mark below it; without that it is permanently uncovered. The earlier candle is
  # interpolation support only — `chart_marked_at_market` never lets it become a point, or the
  # holdings cursor would draw a position the bot did not own yet.
  def chart_candle_window(chart)
    first = chart[:labels].first
    timeframe = optimal_candles_timeframe_for_duration(Time.now.utc - first)
    [first - timeframe, timeframe]
  end

  # The live price, as the grid's final mark, keyed like the holdings. Without it the newest
  # transaction — always inside the still-open candle — would be uncovered, which is where every
  # actively-trading bot sits. Absent when the live lookup failed: then the tail is simply
  # uncovered and keeps its fill mark rather than being marked against an invented endpoint.
  def chart_live_marks(metrics_data, symbol)
    price = metrics_data[:live_prices]&.[](symbol)
    price ? [[metrics_data[:chart][:labels].last, price]] : []
  end

  # chart     - the metrics chart hash (labels + [value, invested] series)
  # grids     - { symbol => [[time, price], ...] } ascending
  # holdings: - ->(i) { { symbol => amount } } held after transaction i
  # cash:     - ->(i) { realized proceeds } after transaction i — cash does not float with price
  def chart_marked_at_market(chart, grids, holdings:, cash:)
    labels = chart[:labels]
    values = chart[:series][0]
    invested = chart[:series][1]
    transaction_times = labels.to_set
    axis = (labels + chart_grid_times(grids, from: labels.first)).uniq.sort

    marked_labels = []
    marked_values = []
    marked_invested = []
    cursor = 0

    axis.each do |time|
      # Post-trade holdings: advance while the NEXT label is still at or before this time, so a
      # transaction's own timestamp lands on that transaction — and where several share a
      # timestamp (an index bot buying eleven assets in one second) on the last of them, which
      # collapses the whole group into one point.
      cursor += 1 while cursor < labels.length - 1 && labels[cursor + 1] <= time
      # A `metrics_with_current_prices` hash cached before this code shipped carries the live
      # label and value but no matching extra_series row, so the live point would read as
      # holding nothing — a −100% chart until that cache expires. Fall back to the last row
      # that exists: the live point holds exactly what the last transaction left. Keyed on
      # MISSING data, never on a zero value, or a bot that legitimately sold out would be
      # redrawn as still holding its position.
      held = holdings.call(cursor)
      held = holdings.call(cursor - 1) if cursor.positive? && held.values.all?(&:nil?)
      at_market = chart_market_value(held, grids, time)

      if at_market
        marked_values << (at_market + cash.call(cursor))
      elsif transaction_times.include?(time)
        marked_values << values[cursor]
      else
        next
      end

      marked_labels << time
      marked_invested << invested[cursor]
    end

    chart.merge(labels: marked_labels, series: [marked_values, marked_invested])
  end

  private

  def chart_grid_times(grids, from:)
    grids.values.flatten(1).filter_map { |time, _price| time if time >= from }
  end

  # nil unless every held symbol has a price at this time — a partially-priced portfolio is not
  # a market value, it is a market value minus whatever could not be priced.
  def chart_market_value(held, grids, time)
    total = 0.to_d
    held.each do |symbol, amount|
      amount = amount.to_d
      next if amount.zero?

      price = chart_grid_price(grids[symbol], time)
      return nil unless price

      total += amount * price
    end
    total
  end

  def chart_grid_price(marks, time)
    return nil if marks.blank? || time < marks.first[0] || time > marks.last[0]

    index = marks.bsearch_index { |mark| mark[0] >= time }
    at = marks[index]
    return at[1] if at[0] == time || index.zero?

    before = marks[index - 1]
    span = at[0] - before[0]
    return before[1] if span.zero?

    # A reading of the line the chart already draws between two real marks — not an observed
    # price. Snapping to the mark below would be systematically stale instead: a daily grid
    # (every bot older than 12.5 days) puts a midday fill up to 24h from its mark.
    # Multiply before dividing: (30 * 3600) / 10800 is exactly 10, while 30 * (3600 / 10800)
    # is 29.999... — BigDecimal division of a repeating fraction truncates at its default scale.
    before[1] + (((at[1] - before[1]) * (time - before[0]).to_d) / span.to_d)
  end
end
