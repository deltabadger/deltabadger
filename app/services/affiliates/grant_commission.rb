module Affiliates
  class GrantCommission
    def call(referral:, payment:)
      return if not_eligible_for_commission?(referral, payment.commission)

      User.transaction do
        referral.reload
        referrer = referral.referrer

        btc_commission_granted = payment.btc_commission
        new_unexported_btc_commission = referrer.unexported_btc_commission + btc_commission_granted

        send_registration_reminder(referrer, btc_commission_granted) if referrer.btc_address.blank?
        referral.referrer.update!(unexported_btc_commission: new_unexported_btc_commission)
      end
    end

    private

    def not_eligible_for_commission?(referral, commission)
      no_commission?(commission) || no_referrer?(referral) || referrer_invalid?(referral)
    end

    def no_commission?(commission)
      !commission.positive?
    end

    def no_referrer?(referral)
      referral.referrer_id.nil?
    end

    def referrer_invalid?(referral)
      !referral.referrer.active?
    end

    def send_registration_reminder(referrer, amount)
      AffiliateMailer.with(
        referrer: referrer,
        amount: amount
      ).registration_reminder.deliver_later
    end
  end
end
