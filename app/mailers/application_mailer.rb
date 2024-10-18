class ApplicationMailer < ActionMailer::Base
  default from: ENV['NOTIFICATIONS_SENDER']
  layout 'mailer'

  def initialize
    super()
    @dca_profit = DcaProfitGetter.call('btc', 1.year.ago)
  end
end
