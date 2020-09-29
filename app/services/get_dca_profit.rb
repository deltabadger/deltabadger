class GetDcaProfit < BaseService
  API_URL = 'https://api.coindesk.com/v1/bpi/historical/close.json'.freeze

  def call(start_date, end_date)
    response = Faraday.get(API_URL, start: start_date, end: end_date)
    return Result::Failure.new(response.body) unless response.status == 200

    response_json = JSON.parse(response.body)
    price_index = response_json.fetch('bpi')
    number_of_days = (end_date - start_date).to_i
    Result::Success.new(calculate_profit(number_of_days, price_index.values))
  end

  private

  def calculate_profit(number_of_days, prices)
    # Assuming 1 BTC per day - number of BTC doesn't affect the percentage
    current_rate = prices.last
    current_value = calculate_current_crypto_value(current_rate, number_of_days)
    total_expenses = prices.sum

    increase = current_value - total_expenses
    increase / total_expenses * 100
  end

  def calculate_current_crypto_value(rate, number_of_btc)
    rate * number_of_btc
  end
end
