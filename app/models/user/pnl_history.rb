# frozen_string_literal: true

# What the account's BOTS have been worth over time — the curve behind the dashboard headline.
#
# The headline is a point: value minus what went in, right now. This is the same subtraction at
# every moment it can be made for, off the same source the bot's OWN chart is drawn from: the
# candle-marked metrics, where the value between two purchases is the holding priced at market
# rather than a flat step from one fill to the next.
#
# That reading is the heavier of the two metrics caches and the warm job does not fill it, so a
# cold one is a WAIT, not a reason to draw the cheap shape: the snapshot says `loading`, the page
# asks for a live pass exactly as it does for a cold headline, and the broadcast brings both.
#
# NOT the tracker's history. That one is the whole account (every connected venue, every manual
# holding, every deposit) and is stored per day in `portfolio_snapshots`. This is the bots alone,
# so the two curves are allowed to disagree — and the last point here IS the headline, because the
# metrics' final label is the live reading the headline is computed from.
class User::PnlHistory
  # The bot types that have a P/L to speak of, as `User#global_pnl_snapshot` counts them.
  MEASURABLE = %i[dca_single_asset? dca_dual_asset? dca_index? dca_multi_asset? signal?].freeze

  # Columns the curve is resampled onto. A sparkline a few hundred pixels wide has nothing to say
  # past this, and every column is two floats in a data attribute.
  MAX_POINTS = 180

  # { result: { percent:, profit_usd:, at:, days: } | nil, loading: Boolean }, on the same three terms
  # as `User#global_pnl_snapshot`: ready, waiting on a cold cache, or nothing to draw.
  # `live:` lets a background job compute what a request may only read.
  def self.snapshot(user, live: false) = new(user, live: live).call

  def initialize(user, live: false)
    @user = user
    @live = live
    @rates = {}
  end

  def call
    tracks = []
    @user.bots.not_deleted.each do |bot|
      next unless MEASURABLE.any? { |type| bot.public_send(type) }

      track = readings(bot)
      # One bot short is the whole account short: a partial curve under a total headline would be
      # a different account's history.
      return { result: nil, loading: true } if track == :cold

      tracks << track if track
    end

    { result: merge(tracks), loading: false }
  end

  private

  # [[time, value_usd, invested_usd], ...] ascending, or :cold while the reading it needs is
  # still being computed.
  def readings(bot)
    metrics = marked_metrics(bot)
    return :cold if metrics == :cold

    labels = metrics&.dig(:chart, :labels)
    return if labels.blank?

    currency = bot.quote_asset&.symbol
    return if currency.blank?

    rate = rate_for(currency)
    return :cold if rate.nil?

    values, invested = metrics.dig(:chart, :series)
    labels.each_with_index.map do |at, i|
      [at, (values[i] || 0).to_d * rate, (invested[i] || 0).to_d * rate]
    end
  end

  # The candle-marked metrics — the bot chart's own ruler. A request may only read them; a cold
  # one there means the curve is not ready yet, which is worth saying only for a bot that has
  # something to draw at all, and the cheap hash is what knows that.
  def marked_metrics(bot)
    return bot.metrics_with_current_prices_and_candles if @live

    cached = bot.metrics_with_current_prices_and_candles_from_cache
    return cached if cached

    bot.metrics_with_current_prices_from_cache&.dig(:chart, :labels).present? ? :cold : nil
  end

  # Every bot holds its last reading until the next one — the marked series already carries the
  # market moves between them — so the account's curve is a sum of step functions, read down a
  # column at a time. Uniform columns, because the plot spaces its points evenly: the axis is
  # time, and a curve drawn from unevenly-spaced readings would stretch quiet weeks and squeeze
  # busy ones.
  def merge(tracks)
    return if tracks.empty?

    first = tracks.map { |track| track.first[0] }.min
    last = tracks.map { |track| track.last[0] }.max
    span = last - first
    return if span <= 0

    # One column MORE than there are readings, and it goes in front of them: the account had made
    # nothing before its first purchase, and the curve has to start there. Without it the curve
    # opens on whatever that first fill marks — a just-bought position priced against a candle
    # grid, which is the spread plus a slice of interpolation below what was paid for it — and an
    # account appears to begin under water before it has done anything.
    columns = (tracks.sum(&:size) + 1).clamp(3, MAX_POINTS)
    step = span / (columns - 2.0)
    cursors = Array.new(tracks.size, 0)
    percent = []
    profit = []
    at_seconds = []

    columns.times do |i|
      # The last column is the last reading itself, not a fraction that lands a float's breadth
      # short of it: that point is the headline, and it has to be exact.
      at = case i
           when 0 then first - step
           when columns - 1 then last
           else first + (step * (i - 1))
           end
      value = invested = 0.to_d
      tracks.each_with_index do |track, index|
        cursors[index] += 1 while cursors[index] + 1 < track.size && track[cursors[index] + 1][0] <= at
        reading = track[cursors[index]]
        next if reading[0] > at # this bot had not started yet

        value += reading[1]
        invested += reading[2]
      end

      at_seconds << at.to_i
      pnl = value - invested
      profit << pnl.to_f
      # A percentage of nothing is not zero percent, but a moment before any money went in has no
      # shape to draw either — it sits on the zero line.
      percent << (invested.positive? ? (pnl / invested).to_f : 0.0)
    end

    { percent: percent, profit_usd: profit, at: at_seconds, days: span / 1.day }
  end

  # One lookup per currency for the whole account, on the same terms as everything else here: a
  # request reads what is cached, a live pass may go and get it.
  def rate_for(currency)
    @rates.fetch(currency.upcase) do
      result = Utilities::Currency.exchange_rate(from: currency, to: 'USD', cache_only: !@live)
      @rates[currency.upcase] = result.success? ? result.data.to_d : nil
    end
  end
end
