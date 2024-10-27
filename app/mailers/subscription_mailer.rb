class SubscriptionMailer < ApplicationMailer
  def subscription_granted
    @payment = params[:payment]

    mail(
      to: @payment.user.email,
      subject: t(
        'subscription_mailer.subscription_granted.subject',
        plan_name: localized_plan_name(@payment.subscription_plan.name)
      )
    )
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
end
