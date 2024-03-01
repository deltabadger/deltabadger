class ApplicationMailer < ActionMailer::Base
  default from: ENV['NOTIFICATIONS_SENDER']
  layout 'mailer'

  def initialize(profit_calculator: GetDcaProfit.new)
    super()
    @dca_profit = profit_calculator.call(1.year.ago, Time.current)
  rescue StandardError
    @dca_profit = Result::Failure.new
  end
end
