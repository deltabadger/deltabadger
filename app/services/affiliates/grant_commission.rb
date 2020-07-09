module Affiliates
  class GrantCommission
    def call(referee:, payment:)
      payment_commission = payment.commission

      return if !payment_commission.positive? || referee.referrer_id.nil?

      User.transaction do
        referee.reload
        referrer = referee.referrer
        max_profit = referrer.max_profit
        current_profit = referee.current_referrer_profit
        commission_available = max_profit - current_profit
        return if commission_available.negative?

        commission_granted = [commission_available, payment_commission].min
        commission_granted_percent = commission_granted / payment_commission
        crypto_commission_granted = payment.crypto_commission * commission_granted_percent
        new_unexported_crypto_commission = referrer.unexported_crypto_commission + crypto_commission_granted
        new_current_profit = current_profit + commission_granted

        referee.update!(current_referrer_profit: new_current_profit)
        referee.referrer.update!(unexported_crypto_commission: new_unexported_crypto_commission)
      end
    end

    private

    def total_commission(referee)
      referee.unexported_commission + referee.exported_commission + referee.paid_commission
    end
  end
end
