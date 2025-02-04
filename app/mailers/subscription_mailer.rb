class SubscriptionMailer < ApplicationMailer
  include LocalesHelper
  helper LocalesHelper

  def subscription_granted
    @payment = params[:payment]

    mail(
      to: @payment.user.email,
      from: 'jan@deltabadger.com',
      subject: 'One more thing'
    ) do |format|
      format.html { render layout: 'plain_mail' }
    end
  end

  def after_wire_transfer
    @payment = params[:payment]

    mail(
      to: @payment.user.email,
      from: 'jan@deltabadger.com',
      subject: "#{localized_plan_name(@payment.subscription_plan.name)} plan granted!"
    ) do |format|
      format.html { render layout: 'plain_mail' }
    end
  end

  def invoice
    @payment = params[:payment]

    mail(to: @payment.user.email, subject: default_i18n_subject)
  end

  def wire_transfer_summary
    @payment = params[:payment]

    mail(
      to: 'jan@deltabadger.com',
      subject: "New wire transfer, ##{@payment.id}"
    )
  end

  private

  helper_method def user_first_name
    return @payment.first_name if @payment.first_name.present?
    return @payment.user.name.split.first if @payment.user.name.present?

    'man'
  end
end
