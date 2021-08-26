module Presenters
  module Payments
    class Upgrade
      attr_reader :payment, :current_plan, :investor_plan, :hodler_plan, :referrer

      def initialize(payment, current_plan, investor_plan, hodler_plan, referrer)
        @payment = payment
        @current_plan = current_plan
        @investor_plan = investor_plan
        @hodler_plan = hodler_plan
        @referrer = referrer
      end

      def current_plan_name
        @current_plan_name ||= current_plan.name
      end

      def available_plans
        @available_plans ||= case current_plan.name
                             when 'saver' then %w[investor hodler]
                             when 'investor' then %w[investor hodler]
                             else %w[hodler]
                             end
      end

      def selected_payment_type
        @selected_payment_type ||= payment.eu? ? 'eu' : 'other'
      end

      def selected_plan_name
        payment.subscription_plan_id == hodler_plan.id ? 'hodler' : 'investor'
      end

      def referrer_discount?
        referrer.present?
      end
    end
  end
end
