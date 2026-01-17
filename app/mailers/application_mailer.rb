class ApplicationMailer < ActionMailer::Base
  default from: ENV.fetch('NOTIFICATIONS_SENDER', 'noreply@localhost')
  layout 'mailers/transactional'

  helper LocalesHelper

  def default_url_options
    { locale: (I18n.locale unless I18n.locale == I18n.default_locale) }
  end

  private

  def set_locale(user)
    I18n.locale = user.try(:locale) || I18n.default_locale
  end
end
