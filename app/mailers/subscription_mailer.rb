class SubscriptionMailer < ApplicationMailer
  include LocalesHelper

  helper LocalesHelper

  has_history

  def subscription_granted
    @payment = params[:payment]
    @user = @payment.user
    set_locale(@user)

    mail(
      to: @user.email,
      from: 'jan@deltabadger.com',
      subject: 'One more thing'
    ) do |format|
      format.html { render layout: 'plain_mail' }
    end
  end

  def after_wire_transfer
    @payment = params[:payment]
    @user = @payment.user
    set_locale(@user)

    mail(
      to: @user.email,
      from: 'jan@deltabadger.com',
      subject: "#{localized_plan_name(@payment.subscription_plan.name)} plan granted!"
    ) do |format|
      format.html { render layout: 'plain_mail' }
    end
  end

  def invoice
    @payment = params[:payment]
    @user = @payment.user
    set_locale(@user)

    mail(to: @user.email, subject: default_i18n_subject)
  end

  def wire_transfer_summary
    @payment = params[:payment]

    mail(
      to: 'jan@deltabadger.com',
      subject: "New wire transfer, ##{@payment.id}"
    )
  end

  def subscription_expiry_warning
    @user = params[:user]
    @days_until_expiry = params[:days_until_expiry]
    @expiry_date = params[:expiry_date]
    @affected_bots = params[:affected_bots] || []
    @current_plan = params[:current_plan]
    @has_research_features = params[:has_research_features] || false
    set_locale(@user)

    mail(to: @user.email, subject: default_i18n_subject)
  end

  def bots_stopped_notification
    @user = params[:user]
    @stopped_bots = params[:stopped_bots] || []
    @expired_plan = params[:expired_plan]
    set_locale(@user)

    mail(to: @user.email, subject: default_i18n_subject)
  end

  def checkout_abandonment
    @user = params[:user]
    @subscription_plan = params[:subscription_plan]
    @has_research_features = params[:has_research_features] || false
    set_locale(@user)

    mail(to: @user.email, subject: default_i18n_subject)
  end

  private

  helper_method def user_first_name
    return @payment.first_name if @payment.first_name.present?
    return @payment.user.name.split.first if @payment.user.name.present?

    'man'
  end
end
