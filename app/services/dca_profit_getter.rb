require 'utilities/time'

class DcaProfitGetter < BaseService
  API_ID_MAP = {
    'bitcoin' => 1713,
    's&p-500' => 65_951
  }.freeze

  def initialize
    @client = FinancialDataApiClient.new
  end

  def call(asset = 'bitcoin', start_date = 4.years.ago)
    Rails.cache.fetch(cache_key(asset, start_date), expires_in: 1.hour) do
      profit_result = query_profit_pcnt_dca(asset, start_date)
      return profit_result if profit_result.failure?

      profit_result
    end
  rescue StandardError
    Result::Failure.new
  end

  private

  def cache_key(asset, start_date)
    days_since_start_date = (Time.current.to_date - start_date.to_date).to_i
    "dca_profit_#{asset}_#{days_since_start_date}"
  end

  def query_profit_pcnt_dca(asset, start_date)
    time_series_result = @client.time_series(symbol: API_ID_MAP[asset], timeframe: '1d', start: (start_date - 1.day).iso8601)
    return time_series_result if time_series_result.failure?

    close_prices = time_series_result.data.map { |x| x[4] }
    Result::Success.new(calculate_profit_pcnt(close_prices))
  end

  def calculate_profit_pcnt(prices)
    # Assuming 10$ per day - amount doesn't affect the percentage
    dollars_per_day = 10
    asset_amount_purchased = prices.sum { |price| dollars_per_day / price }
    investment_current_value_usd = prices.last * asset_amount_purchased
    invested_amount_usd = prices.size * dollars_per_day

    (investment_current_value_usd - invested_amount_usd) / invested_amount_usd.to_f
  end
end
