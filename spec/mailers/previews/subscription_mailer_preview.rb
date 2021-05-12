# Preview all emails at http://localhost:3000/rails/mailers/subscription_mailer
class SubscriptionMailerPreview < ActionMailer::Preview
  def after_wire_transfer
    SubscriptionMailer.after_wire_transfer
  end
end
