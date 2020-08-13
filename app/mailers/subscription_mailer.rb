class SubscriptionMailer < ApplicationMailer
  def subscription_granted
    @user = params[:user]
    @subscription_plan = params[:subscription_plan]

    mail(to: @user.email, subject: "#{@subscription_plan.display_name} plan granted")
  end

  def invoice
    @user = params[:user]
    @payment = params[:payment]

    mail(to: @user.email, subject: 'Deltabadger Payment')
  end
end
