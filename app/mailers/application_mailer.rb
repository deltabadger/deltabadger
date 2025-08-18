class ApplicationMailer < ActionMailer::Base
  default from: ENV.fetch('NOTIFICATIONS_SENDER')
  layout 'mailer'

  helper LocalesHelper

  before_action :set_show_dca_profit

  def default_url_options
    { locale: (I18n.locale unless I18n.locale == I18n.default_locale) }
  end

  private

  def set_show_dca_profit
    @show_dca_profit = true
  end
end
