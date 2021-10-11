class ApplicationMailer < ActionMailer::Base
  default from: ENV['NOTIFICATIONS_SENDER']
  layout 'mailer'

  def initialize(profit_calculator: GetDcaProfit.new)
    super()
    today = Time.now
    @dca_profit = profit_calculator.call(today - 365.days, today)
  rescue StandardError
    @dca_profit = Result::Failure.new
  end
end
