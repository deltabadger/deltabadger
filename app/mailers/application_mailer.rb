class ApplicationMailer < ActionMailer::Base
  default from: ENV['NOTIFICATIONS_SENDER']
  layout 'mailer'

  helper LocalesHelper

  before_action :set_show_dca_profit

  private

  def set_show_dca_profit
    @show_dca_profit = true
  end
end
