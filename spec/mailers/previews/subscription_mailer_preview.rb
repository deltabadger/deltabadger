# Preview all emails at http://localhost:3000/rails/mailers/subscription_mailer
class SubscriptionMailerPreview < ActionMailer::Preview
  def subscription_granted
    SubscriptionMailer.with(payment: sample_payment).subscription_granted
  end

  def subscription_granted_no_payment_name
    payment = sample_payment
    payment.first_name = nil
    SubscriptionMailer.with(payment: payment).subscription_granted
  end

  def subscription_granted_no_names
    payment = sample_payment
    payment.first_name = nil
    payment.user.name = nil
    SubscriptionMailer.with(payment: payment).subscription_granted
  end

  def after_wire_transfer
    SubscriptionMailer.with(payment: sample_payment).after_wire_transfer
  end

  def invoice
    SubscriptionMailer.with(payment: sample_payment).invoice
  end

  def wire_transfer_summary
    SubscriptionMailer.with(payment: sample_payment).wire_transfer_summary
  end

  private

  def sample_payment
    user = User.new(
      email: 'test@example.com',
      name: 'Mathias'
    )

    subscription_plan = SubscriptionPlan.new(
      name: 'pro'
    )

    subscription_plan_variant = SubscriptionPlanVariant.new(
      subscription_plan: subscription_plan,
      years: 1
    )

    Payment.new(
      user: user,
      subscription_plan_variant: subscription_plan_variant,
      payment_type: 'wire',
      status: 'unpaid',
      total: 999.99,
      currency: 'USD',
      first_name: 'Mathias',
      last_name: 'User',
      country: 'US',
      commission: 0,
      discounted: false
    )
  end
end
