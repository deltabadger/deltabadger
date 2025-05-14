class BotAlertsMailer < ApplicationMailer
  def notify_about_error
    @user = params[:user]
    @errors = params[:errors].to_sentence
    @bot = params[:bot]
    @label = @bot.label
    @exchange_name = Exchange.find(@bot.exchange_id).name.upcase

    mail(to: @user.email, subject: default_i18n_subject)
  end

  def notify_about_restart
    @user = params[:user]
    @restart_at = params[:restart_at]
    @errors = params[:errors].to_sentence
    @bot = params[:bot]
    @label = @bot.label
    @exchange_name = Exchange.find(@bot.exchange_id).name.upcase

    mail(to: @user.email, subject: default_i18n_subject)
  end

  def end_of_funds
    @user = params[:user]
    @bot = params[:bot]
    @quote = params[:quote]
    @label = @bot.label
    @exchange_name = Exchange.find(@bot.exchange_id).name

    mail(to: @user.email, subject: default_i18n_subject)
  end

  def successful_webhook_bot_transaction
    @user = params[:user]
    @bot = params[:bot]
    @base = params[:base]
    @quote = params[:quote]
    @bot_name = params[:bot_name]
    @type = params[:type]
    @price = params[:price]
    @exchange_name = @bot.exchange.name

    mail(to: @user.email, subject: default_i18n_subject)
  end

  def stopped_by_quote_amount_limit
    @user = params[:user]
    @label = params[:label]
    @amount = params[:amount]
    @quote = params[:quote]

    mail(to: @user.email, subject: t('.subject', label: @label))
  end
end
