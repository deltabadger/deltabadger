class BotAlertsMailer < ApplicationMailer
  def notify_about_error
    @user = params[:user]
    @errors = params[:errors]
    @bot = params[:bot]

    mail(to: @user.email, subject: 'Something went wrong!')
  end
end
