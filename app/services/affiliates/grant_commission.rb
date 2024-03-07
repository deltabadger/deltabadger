module Affiliates
  class GrantCommission
    def call(referee:, payment:)
      payment_commission = payment.commission
      return if not_eligible_for_commission?(referee, payment_commission)

      User.transaction do
        referee.reload
        referrer = referee.referrer
        max_profit = referrer.max_profit
        current_profit = referee.current_referrer_profit
        commission_available = max_profit - current_profit
        return unless commission_available.positive?

        commission_granted = [commission_available, payment_commission].min
        commission_granted_percent = commission_granted / payment_commission
        crypto_commission_granted = payment.crypto_commission * commission_granted_percent
        new_unexported_crypto_commission =
          referrer.unexported_crypto_commission + crypto_commission_granted
        new_current_profit = current_profit + commission_granted

        send_registration_reminder(referrer, crypto_commission_granted) if referrer.btc_address.blank?
        referee.update!(current_referrer_profit: new_current_profit)
        referee.referrer.update!(unexported_crypto_commission: new_unexported_crypto_commission)
      end
    end

    private

    def not_eligible_for_commission?(referee, commission)
      no_commission?(commission) || no_referrer?(referee) || referrer_invalid?(referee)
    end

    def no_commission?(payment_commission)
      !payment_commission.positive?
    end

    def no_referrer?(referee)
      referee.referrer_id.nil?
    end

    def referrer_invalid?(referee)
      !referee.referrer.active?
    end

    def total_commission(referee)
      referee.unexported_commission + referee.exported_commission + referee.paid_commission
    end

    def send_registration_reminder(referrer, amount)
      AffiliateMailer.with(
        referrer: referrer,
        amount: amount
      ).registration_reminder.deliver_later
    end
  end
end
