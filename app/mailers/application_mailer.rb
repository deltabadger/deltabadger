class ApplicationMailer < ActionMailer::Base
  default from: ENV['NOTIFICATIONS_SENDER']
  layout 'mailer'

  def initialize(profit_calculator: GetDcaProfit.new)
    super()
    today = Date.today
    @dca_profit = profit_calculator.call(today - 365, today)
  end
end
