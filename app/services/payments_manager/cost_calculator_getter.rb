module PaymentsManager
  class CostCalculatorGetter < BaseService
    def call(payment:, user:)
      referrer = user.eligible_referrer
      discount_percent = referrer&.discount_percent || 0
      commission_percent = referrer&.commission_percent || 0

      PaymentsManager::CostCalculatorFactory.call(
        from_eu: payment.eu?,
        vat: VatRate.find_by!(country: payment.country).vat,
        subscription_plan: payment.subscription_plan,
        current_plan: user.subscription.subscription_plan,
        days_left: user.plan_days_left,
        discount_percent: discount_percent,
        commission_percent: commission_percent
      )
    end
  end
end
