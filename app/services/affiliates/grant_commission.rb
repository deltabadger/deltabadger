module Affiliates
  class GrantCommission
    def call(referral:, payment:)
      payment_commission = payment.commission
      return if not_eligible_for_commission?(referral, payment_commission)

      User.transaction do
        referral.reload
        referrer = referral.referrer
        max_profit = referrer.max_profit
        current_profit = referral.current_referrer_profit
        commission_available = max_profit - current_profit
        return unless commission_available.positive?

        commission_granted = [commission_available, payment_commission].min
        commission_granted_percent = commission_granted / payment_commission
        btc_commission_granted = payment.btc_commission * commission_granted_percent
        new_unexported_btc_commission =
          referrer.unexported_btc_commission + btc_commission_granted
        new_current_profit = current_profit + commission_granted

        send_registration_reminder(referrer, btc_commission_granted) if referrer.btc_address.blank?
        referral.update!(current_referrer_profit: new_current_profit)
        referral.referrer.update!(unexported_btc_commission: new_unexported_btc_commission)
      end
    end

    private

    def not_eligible_for_commission?(referral, commission)
      no_commission?(commission) || no_referrer?(referral) || referrer_invalid?(referral)
    end

    def no_commission?(payment_commission)
      !payment_commission.positive?
    end

    def no_referrer?(referral)
      referral.referrer_id.nil?
    end

    def referrer_invalid?(referral)
      !referral.referrer.active?
    end

    def total_commission(referral)
      referral.unexported_commission + referral.exported_commission + referral.paid_commission
    end

    def send_registration_reminder(referrer, amount)
      AffiliateMailer.with(
        referrer: referrer,
        amount: amount
      ).registration_reminder.deliver_later
    end
  end
end
