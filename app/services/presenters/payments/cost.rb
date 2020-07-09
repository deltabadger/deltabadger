module Presenters
  module Payments
    class Cost
      attr_reader :calculated_cost

      def initialize(calculated_cost)
        @calculated_cost = calculated_cost
      end

      def base_price
        format_price(calculated_cost[:base_price])
      end

      def vat
        format_price(calculated_cost[:vat])
      end

      def discount
        format_price(calculated_cost[:discount])
      end

      def price_with_vat
        format_price(calculated_cost[:price_with_vat])
      end

      def total_price
        format_price(calculated_cost[:total_price])
      end

      private

      def format_price(price)
        format('%0.02f', price)
      end
    end
  end
end
