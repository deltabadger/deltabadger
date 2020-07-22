module Presenters
  module Payments
    class Cost
      attr_reader :cost_calculator

      def initialize(cost_calculator)
        @cost_calculator = cost_calculator
      end

      def base_price
        format_price(cost_calculator.base_price)
      end

      def vat
        format_price(cost_calculator.vat)
      end

      def flat_discount_with_vat
        format_price(cost_calculator.flat_discount_with_vat)
      end

      def discount
        format_price(cost_calculator.discount)
      end

      def base_price_with_vat
        format_price(cost_calculator.base_price_with_vat)
      end

      def total_price
        format_price(cost_calculator.total_price)
      end

      private

      def format_price(price)
        format('%0.02f', price)
      end
    end
  end
end
