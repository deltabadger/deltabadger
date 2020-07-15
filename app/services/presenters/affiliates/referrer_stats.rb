module Presenters
  module Affiliates
    class ReferrerStats
      attr_reader :referrer

      def initialize(referrer)
        @referrer = referrer
      end

      def referee_count
        @referee_count ||= referrer.referees.size
      end

      def paid_commission
        @paid_commission ||= format_btc(referrer.paid_crypto_commission)
      end

      def unpaid_commission
        @unpaid_commission ||= format_btc(referrer.unexported_crypto_commission + referrer.exported_crypto_commission)
      end

      def format_btc(amount)
        format('%0.8g BTC', amount)
      end
    end
  end
end
