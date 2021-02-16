class BotAlertsMailer < ApplicationMailer
  def notify_about_error
    @user = params[:user]
    @errors = params[:errors]
    @bot = params[:bot]
    @exchange_name = Exchange.find(@bot.exchange_id).name.upcase

    mail(to: @user.email, subject: 'Something went wrong 😵')
  end

  def notify_about_restart
    @user = params[:user]
    @delay = params[:delay]
    @errors = params[:errors]
    @bot = params[:bot]
    @exchange_name = Exchange.find(@bot.exchange_id).name.upcase

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

  def first_month_ending_soon
    @user = params[:user]
    @bot = params[:bot]

    mail(to: @user.email, subject: 'First month trial ending soon!')
  end
end
