class ApplicationMailer < ActionMailer::Base
  default from: ENV['NOTIFICATIONS_SENDER']
  layout 'mailer'

  def initialize(profit_calculator: GetDcaProfit.new)
    super()
    @dca_profit = calculate_profit(profit_calculator)
  end

  private

  def calculate_profit(profit_calculator)
    today = Date.today
    profit_result = profit_calculator.call(today - 365, today)
    return 0 unless profit_result.success?

    profit_result.data.to_i
  end
end
