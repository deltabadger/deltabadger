class ApplicationMailer < ActionMailer::Base
  default from: ENV['NOTIFICATIONS_SENDER']
  layout 'mailer'

  def initialize
    super()
    @dca_profit = DcaProfitGetter.call('bitcoin', 1.year.ago).data * 100
  rescue StandardError
    @dca_profit = Result::Failure.new
  end
end
