class ApplicationMailer < ActionMailer::Base
  default from: ENV['NOTIFICATIONS_SENDER']
  layout 'mailer'

  def initialize
    super()
    @dca_profit = DcaProfitGetter.call(1.year.ago, Time.current)
  rescue StandardError
    @dca_profit = Result::Failure.new
  end
end
