class CustomDeviseMailer < Devise::Mailer
  layout 'mailers/transactional'

  helper LocalesHelper

  default template_path: 'devise/mailer'

  def email_already_taken(email)
    @resource = User.find_by(email:)

    mail(to: email, subject: t('devise.mailer.email_already_taken.subject'))
  end

  def confirm_email(record, token)
    @resource = record
    @token = token

    mail(to: @resource.unconfirmed_email)
  end
end
