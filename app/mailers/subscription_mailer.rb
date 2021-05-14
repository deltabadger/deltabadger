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

  def after_wire_transfer
    @user = params[:user]
    @subscription_plan = params[:subscription_plan]
    @name = params[:name]

    mail(
      to: @user.email,
      subject: @subscription_plan.display_name + ' plan granted!'
    )
  end

  def invoice
    @user = params[:user]
    @payment = params[:payment]

    mail(to: @user.email, subject: default_i18n_subject)
  end

  def wire_transfer_summary
    @email = params[:email]
    @subscription_plan = params[:subscription_plan]
    @first_name = params[:first_name]
    @last_name = params[:last_name]
    @country = params[:country]

    mail(
      to: 'mailjanka@deltabadger.com',
      subject: 'New wire transfer'
    )
  end
end
