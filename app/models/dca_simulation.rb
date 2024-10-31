require 'utilities/time'

class DcaSimulation
  API_ID_MAP = {
    'btc' => 1713,     # Bitcoin
    'qqq' => 51_788,   # Invesco QQQ Trust
    'gspc' => 65_951,  # S&P 500 Index
    'gdaxi' => 65_946, # DAX Index
    'gld' => 65_437,   # XAUUSD - Gold Spot US Dollar
    'ndx' => 66_058,   # NASDAQ-100 Index
    'usd' => 65_419    # US Dollar
  }.freeze

  def initialize(asset:, interval:, amount:, target_profit:)
    @asset = asset
    @interval = interval
    @amount = amount
    @target_profit = target_profit
    @client = FinancialDataApiClient.new
  end

  def perform
    first_investment_date(time_series)
  end

  private

  def time_series
    expires_in = Utilities::Time.seconds_to_midnight_utc.seconds + 5.minutes
    Rails.cache.fetch("all_time_series_#{@asset}", expires_in: expires_in) do
      time_series_result = @client.time_series(symbol: API_ID_MAP[@asset], timeframe: '1d')
      return time_series_result if time_series_result.failure?

      time_series_result.data
    end
  end

  def first_investment_date(data_points)
    return Date.today if @amount >= @target_profit

    next_time_check = Date.today.prev_month
    current_price = data_points.last[4]
    bought_amount = 0
    amount_of_intervals = 0
    first_buy_price = current_price
    data_points.reverse.each do |datetime, _open, _high, _low, close|
      date = Date.parse(datetime)
      next if date > next_time_check

      bought_amount += @amount / close
      return date if bought_amount * current_price >= @target_profit

      next_time_check = next_time_check.prev_month
      amount_of_intervals += 1
      first_buy_price = close
    end

    # if we reach this point, we didn't reach the target profit after DCAing through all the given data
    # we assume previous data would have similar average performance
    avg_monthly_return = (current_price / first_buy_price)**(1.0 / amount_of_intervals) - 1
    virtual_close = first_buy_price / (1 + avg_monthly_return)
    while bought_amount * current_price < @target_profit
      bought_amount += @amount / virtual_close
      return next_time_check if bought_amount * current_price >= @target_profit

      virtual_close /= 1 + avg_monthly_return
      next_time_check = next_time_check.prev_month
    end
  end
end
