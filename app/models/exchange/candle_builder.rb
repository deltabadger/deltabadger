module Exchange::CandleBuilder
  extend ActiveSupport::Concern

  def build_candles_from_candles(candles:, timeframe:)
    # Warning: candles must have a smaller timeframe than the one we're building, otherwise it returns the same candles

    grouped_candles(candles, timeframe).map do |date, group|
      next if date < candles.first[0]

      [
        date,                        # Date (start of the timeframe period)
        group.first[1],              # Open (first candle's open)
        group.map { |c| c[2] }.max,  # High (max high in group)
        group.map { |c| c[3] }.min,  # Low (min low in group)
        group.last[4],               # Close (last candle's close)
        group.sum { |c| c[5] }       # Volume (sum of volumes)
      ]
    end.compact
  end

  private

  def grouped_candles(candles, timeframe)
    candles.group_by do |candle|
      if timeframe == 1.week
        candle[0].beginning_of_week
      elsif timeframe == 1.month
        candle[0].beginning_of_month
      else
        timeframe_seconds = timeframe.to_i
        timestamp = candle[0].to_i
        period_start = (timestamp / timeframe_seconds) * timeframe_seconds
        Time.at(period_start).utc
      end
    end
  end
end
