# frozen_string_literal: true

# One ruler for the bot chart: every point valued at MARKET.
#
# `metrics` can only mark holdings at the price of the transaction that produced them — it has
# no prices of its own — so a transaction point used to re-price the WHOLE position at that one
# fill, while the candle points around it priced the same holdings at market. Two rulers on one
# line: a fill away from market showed as a dip at the exact moment the bot got a good fill, and
# every buy became a sawtooth vertex.
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

  # How finely the buy marks are worth shipping. The marks collapse into stacks by PIXEL
  # proximity in the browser, and the plot is a few hundred CSS pixels wide, so two marks landing
  # in the same 1/500th of the history are inside the same stack at any width this app renders —
  # shipping both only costs bytes. It bounds the payload, which is otherwise one tuple per fill
  # for the life of the bot: a five-minute smart-interval bot buying twenty assets makes millions
  # of them, and every one would be serialized into a data attribute on every render AND on every
  # metrics broadcast.
  MARK_BUCKETS = 500

  # `[[time, symbol, amount, quote, fills], ...]` oldest first: the buys the chart marks under
  # the plot, each carrying what it bought so the mark can say so when hovered.
  #
  # The SAME rows `metrics` counts, and for the same reason — a mark is a claim that the bot
  # bought something there. An order still open, or cancelled before it filled, carries a price
  # and no execution: it would put a logo under a purchase that never happened. Filtered in Ruby
  # through `Transaction.confirmed_exec_amounts` rather than in SQL, because a closed row is
  # allowed to have its execution missing and is read back from amount * price — one branch, in
  # one place, rather than the same rule spelled twice in two languages.
  def chart_buy_marks
    marks = transactions.submitted.buy.order(:created_at)
                        .pluck(:created_at, :base, :price, :amount, :amount_exec, :quote_amount_exec, :external_status)
                        .filter_map do |at, base, price, amount, amount_exec, quote_amount_exec, external_status|
      amount_exec, quote_amount_exec =
        Transaction.confirmed_exec_amounts(external_status, price, amount, amount_exec, quote_amount_exec)
      next if price.blank? || amount_exec.blank? || quote_amount_exec.blank?
      next if amount_exec.zero? || quote_amount_exec.zero?

      # Amounts as EXECUTED, not as requested — `price` on the row is what was asked for, and a
      # market order rarely gets exactly that. The fill price the tooltip shows is derived from
      # these two, so it is the price the money actually moved at.
      [at, base, amount_exec.to_d, quote_amount_exec.to_d, 1]
    end
    chart_thinned_marks(marks)
  end

  # One mark per symbol per bucket, timed at the FIRST in each — so the first and last buys of
  # the history both survive, and a symbol that only ever traded once is never thinned away.
  # Applied only when there are more marks than buckets: the ordinary bot pays nothing for this.
  #
  # SUMMED, not sampled. The marks say what they bought, and a hovered mark that dropped half the
  # fills underneath it would understate a purchase — a number being wrong is worse than a mark
  # being a pixel off. Bucketed marks are ordered by their first fill because a Hash keeps
  # insertion order and the rows arrive ascending.
  def chart_thinned_marks(marks)
    return marks if marks.size <= MARK_BUCKETS

    first = marks.first[0]
    span = marks.last[0] - first
    return marks if span.zero?

    marks.each_with_object({}) do |(at, base, amount, quote, fills), buckets|
      bucket = buckets[[((at - first) / span * MARK_BUCKETS).floor, base]] ||= [at, base, 0.to_d, 0.to_d, 0]
      bucket[2] += amount
      bucket[3] += quote
      bucket[4] += fills
    end.values
  end

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
  # basis:    - ->(i) { { symbol => cost basis } } after transaction i. Given, the chart also
  #             carries a per-symbol split (:assets) for the holdings tables to hover against.
  #             Omitted where the split would only duplicate the portfolio line — a single-asset
  #             bot's one holding IS the whole chart.
  def chart_marked_at_market(chart, grids, holdings:, cash:, basis: nil)
    labels = chart[:labels]
    values = chart[:series][0]
    invested = chart[:series][1]
    transaction_times = labels.to_set
    axis = (labels + chart_grid_times(grids, from: labels.first)).uniq.sort

    marked_labels = []
    marked_values = []
    marked_invested = []
    marked_split = []
    marked_basis = []
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
      row = cursor
      held = holdings.call(row)
      if row.positive? && held.values.all?(&:nil?)
        row -= 1
        held = holdings.call(row)
      end
      split = chart_market_values(held, grids, time)
      at_market = split&.values&.sum

      if at_market
        marked_values << (at_market + cash.call(cursor))
      elsif transaction_times.include?(time)
        marked_values << values[cursor]
      else
        next
      end

      marked_labels << time
      marked_invested << invested[cursor]
      next unless basis

      # nil where the point kept its fill mark: the portfolio total survives there, but no
      # per-symbol split does, and inventing one would put an asset at zero.
      marked_split << split
      marked_basis << basis.call(row)
    end

    marked = chart.merge(labels: marked_labels, series: [marked_values, marked_invested])
    basis ? marked.merge(assets: chart_asset_series(marked_split, marked_basis)) : marked
  end

  private

  def chart_grid_times(grids, from:)
    grids.values.flatten(1).filter_map { |time, _price| time if time >= from }
  end

  # { symbol => value at market }, or nil unless every held symbol has a price at this time — a
  # partially-priced portfolio is not a market value, it is a market value minus whatever could
  # not be priced. A symbol held at zero needs no price: it is worth zero at any of them.
  def chart_market_values(held, grids, time)
    values = {}
    held.each do |symbol, amount|
      amount = amount.to_d
      if amount.zero?
        values[symbol] = 0.to_d
        next
      end

      price = chart_grid_price(grids[symbol], time)
      return nil unless price

      values[symbol] = amount * price
    end
    values
  end

  # The per-point splits transposed into one series per symbol, each parallel with the marked
  # labels — what the chart swaps in when a holdings row is hovered.
  #
  # ponytail: every symbol ships its whole series with the page — 57 KB of JSON on a 20-coin index
  # over 246 points, so a few hundred on a wide one with years of daily candles behind it. Fetch
  # the hovered symbol on demand if that ever bites.
  def chart_asset_series(splits, basis)
    splits.compact.flat_map(&:keys).uniq.index_with do |symbol|
      { value: splits.map { |split| split && (split[symbol] || 0) },
        invested: basis.map { |held| held[symbol] || 0 } }
    end
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
