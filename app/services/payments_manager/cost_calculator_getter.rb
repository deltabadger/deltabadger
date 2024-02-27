module PaymentsManager
  class CostCalculatorGetter < BaseService
    def call(payment:, user:)
      PaymentsManager::CostCalculatorFactory.call(
        from_eu: payment.eu?,
        vat: VatRate.find_by!(country: payment.country).vat,
        subscription_plan: payment.subscription_plan,
        user: user
      )
    end
  end
end
