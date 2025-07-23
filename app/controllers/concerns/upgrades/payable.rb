module Upgrades::Payable
  extend ActiveSupport::Concern

  private

  def new_payment_for(plan_name:, days:, type:, country:, first_name: nil, last_name: nil, birth_date: nil)
    subscription_plan = SubscriptionPlan.find_by(name: plan_name)
    variant = SubscriptionPlanVariant.includes(:subscription_plan).find_by(
      subscription_plan: subscription_plan,
      days: days
    ) || SubscriptionPlanVariant.includes(:subscription_plan).find_by(
      subscription_plan: subscription_plan,
      days: nil
    )
    current_user.payments.new(
      status: :unpaid,
      type: type,
      subscription_plan_variant: variant,
      country: country,
      currency: country != VatRate::NOT_EU ? :eur : :usd,
      first_name: first_name,
      last_name: last_name,
      birth_date: birth_date
    )
  end
end
