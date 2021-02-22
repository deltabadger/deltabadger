class SubscriptionMailer < ApplicationMailer
  def subscription_granted
    @user = params[:user]
    @subscription_plan = params[:subscription_plan]

    mail(
      to: @user.email,
      subject: I18n.t(
        'subscription_mailer.subscription_granted.subject',
        plan_name: @subscription_plan.display_name
      )
    )
  end

  def invoice
    @user = params[:user]
    @payment = params[:payment]

    mail(to: @user.email, subject: default_i18n_subject)
  end
end
