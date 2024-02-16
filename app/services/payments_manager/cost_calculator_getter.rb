module PaymentsManager
  class CostCalculatorGetter < ApplicationService

    def initialize(payment:, user:)
      @payment = payment
      @user = user
    end

    def call
      subscription_plan = @payment.subscription_plan
      referrer = @user.eligible_referrer
      discount_percent = referrer&.discount_percent || 0
      commission_percent = referrer&.commission_percent || 0

      current_plan = @user.subscription.subscription_plan

      vat = VatRate.find_by!(country: @payment.country).vat

      PaymentsManager::CostCalculatorFactory.call(
        eu: @payment.eu?,
        vat: vat,
        subscription_plan: subscription_plan,
        current_plan: current_plan,
        days_left: @user.plan_days_left,
        discount_percent: discount_percent,
        commission_percent: commission_percent
      )
    end
  end
end
