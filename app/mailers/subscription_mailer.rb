class SubscriptionMailer < ApplicationMailer
  def unlimited_granted
    @user = params[:user]

    mail(to: @user.email, subject: 'Unlimited granted')
  end

  def invoice
    @user = params[:user]
    @payment = params[:payment]

    mail(to: @user.email, subject: 'Deltabadger Payment')
  end
end
