class GetDcaProfit < BaseService
  API_URL = 'https://api.coinpaprika.com/v1/coins/btc-bitcoin/ohlcv/historical'.freeze
  CACHE_KEY = 'dca_profit'.freeze

  def call(start_date, end_date)
    return Rails.cache.read(CACHE_KEY) if Rails.cache.exist?(CACHE_KEY)

    profit = query_profit_dca(start_date, end_date)
    Rails.cache.write(CACHE_KEY, profit, expires_in: 1.day) if profit.success?
    profit
  rescue StandardError
    Result::Failure.new
  end

  private

  def query_profit_dca(start_date, end_date)
    response = Faraday.get(API_URL, start: start_date.to_i, end: end_date.to_i)
    return Result::Failure.new unless response.status == 200

    response_json = JSON.parse(response.body)
    price_index = response_json.map { |x| x.fetch('close') }
    Result::Success.new(calculate_profit(price_index))
  end

  def calculate_profit(prices)
    number_of_days = prices.length
    # Assuming 10$ per day - amount doesn't affect the percentage
    dollars_per_day = 10
    total_purchased_btc = calculate_purchased_btc(prices, dollars_per_day)
    current_rate = prices.last
    current_value = calculate_current_crypto_value(current_rate, total_purchased_btc)
    total_expenses = number_of_days * dollars_per_day

    increase = current_value - total_expenses
    increase / total_expenses * 100
  end

  def calculate_purchased_btc(prices, amount)
    prices.inject(0) do |sum, price|
      sum + amount / price
    end
  end

  def calculate_current_crypto_value(rate, number_of_btc)
    rate * number_of_btc
  end
end
