class BotAlertsMailer < ApplicationMailer
  def notify_about_error
    @user = params[:user]
    @errors = params[:errors]
    @bot = params[:bot]

    mail(to: @user.email, subject: 'Something went wrong 😵')
  end

  def notify_about_restart
    @user = params[:user]
    @delay = params[:delay]
    @errors = params[:errors]
    @bot = params[:bot]

    mail(to: @user.email, subject: 'Oups! Next try… 🧐')
  end

  def limit_reached
    @user = params[:user]
    @bot = params[:bot]

    mail(to: @user.email, subject: 'You\'ve reached the limit 🥳')
  end

  def limit_almost_reached
    @user = params[:user]
    @bot = params[:bot]

    mail(to: @user.email, subject: 'Limit almost reached ⌛')
  end
end
