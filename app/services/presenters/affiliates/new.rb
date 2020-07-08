module Presenters
  module Affiliates
    class New
      attr_reader :affiliate

      def initialize(affiliate)
        @affiliate = affiliate
      end

      def individual?
        affiliate.type.nil? || affiliate.type == 'individual'
      end

      def percent_step
        0.05
      end

      def bonus_percent
        Affiliate::DEFAULT_BONUS_PERCENT
      end

      def discount_percent
        affiliate.discount_percent || Affiliate::DEFAULT_DISCOUNT_PERCENT
      end

      def earn_percent
        (Affiliate::DEFAULT_BONUS_PERCENT - discount_percent)
      end

      def discount_percent_preview
        (discount_percent * 100).round
      end

      def earn_percent_preview
        (earn_percent * 100).round
      end
    end
  end
end
