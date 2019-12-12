class SubscriptionMailer < ApplicationMailer
  def unlimited_granted
    @user = params[:user]

    mail(to: @user.email, subject: 'Unlimited granted')
  end
end
