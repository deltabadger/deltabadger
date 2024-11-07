require 'utilities/time'

class DcaProfitGetter < BaseService
  API_ID_MAP = {
    'btc' => 1713, # Bitcoin
    'gspc' => 65_951,  # S&P 500 Index
    'vti' => 61_914,   # Vanguard Total Stock Market Index Fund ETF Shares
    'vt' => 61_885,    # Vanguard Total World Stock Index Fund ETF Shares
    'qqq' => 51_788,   # Invesco QQQ Trust
    'gld' => 65_437,   # XAUUSD - Gold Spot US Dollar
    'ita' => 37_549    # iShares U.S. Aerospace & Defense ETF
  }.freeze

  def initialize
    @client = FinancialDataApiClient.new
  end

  def call(asset = 'btc', start_date = 4.years.ago)
    expires_in = Utilities::Time.seconds_to_midnight_utc.seconds + 5.minutes
    Rails.cache.fetch(cache_key(asset, start_date), expires_in: expires_in) do
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
