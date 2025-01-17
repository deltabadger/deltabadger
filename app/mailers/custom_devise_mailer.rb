class CustomDeviseMailer < Devise::Mailer
  layout 'mailer'

  before_action :set_show_dca_profit

  helper LocalesHelper

  default template_path: 'devise/mailer'

  def email_already_taken(email)
    @resource = User.find_by(email: email)
    mail(to: email, subject: t('devise.mailer.email_already_taken.subject'))
  end

  private

  def set_show_dca_profit
    @show_dca_profit = false
  end
end
