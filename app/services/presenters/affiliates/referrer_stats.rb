module Presenters
  module Affiliates
    class ReferrerStats
      attr_reader :referrer

      def initialize(referrer)
        @referrer = referrer
      end

      def referral_count
        @referral_count ||= referrer.referrals.size
      end

      def paid_commission
        @paid_commission ||= format_btc(referrer.paid_btc_commission)
      end

      def unpaid_commission
        @unpaid_commission ||= format_btc(
          referrer.unexported_btc_commission + referrer.exported_btc_commission
        )
      end

      def format_btc(amount)
        format('%.8f', amount)
      end
    end
  end
end
