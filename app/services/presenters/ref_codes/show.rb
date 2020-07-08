module Presenters
  module RefCodes
    class Show
      attr_reader :affiliate

      def initialize(affiliate)
        @affiliate = affiliate
      end

      def valid?
        affiliate.present?
      end

      def code
        affiliate&.code
      end

      def discount_percent
        (affiliate.discount_percent * 100).round
      end

      def info?
        affiliate.visible_name.present? && affiliate.visible_link.present?
      end

      def visible_name
        affiliate.visible_name
      end

      def visible_link
        affiliate.visible_link
      end
    end
  end
end
