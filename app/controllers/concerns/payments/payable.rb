module Payments::Payable
  extend ActiveSupport::Concern

  private

  def new_payment_for(plan_name, years, type, country)
    subscription_plan = SubscriptionPlan.find_by(name: plan_name)
    variant = SubscriptionPlanVariant.includes(:subscription_plan).find_by(
      subscription_plan: subscription_plan,
      years: years
    ) || SubscriptionPlanVariant.includes(:subscription_plan).find_by(
      subscription_plan: subscription_plan,
      years: nil
    )
    current_user.payments.new(
      status: :unpaid,
      type: type,
      subscription_plan_variant: variant,
      country: country,
      currency: country != VatRate::NOT_EU ? :EUR : :USD
    )
  end
end
