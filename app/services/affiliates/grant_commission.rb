module Affiliates
  class GrantCommission
    def call(referee:, payment:)
      return if referee.referrer_id.nil?

      payment_commission = payment.commission

      User.transaction do
        referee.reload
        max_profit = referee.referrer.max_profit
        commission_available = [max_profit - total_commission(referee), 0].max
        commission_granted = [commission_available, payment_commission].min
        new_unexported_commission = referee.unexported_commission + commission_granted
        referee.update!(unexported_commission: new_unexported_commission)
      end
    end

    private

    def total_commission(referee)
      referee.unexported_commission + referee.exported_commission + referee.paid_commission
    end
  end
end
