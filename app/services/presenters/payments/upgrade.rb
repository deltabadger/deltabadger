module Presenters
  module Payments
    class Upgrade
      attr_reader :payment, :current_plan, :investor_plan, :hodler_plan, :referrer

      def initialize(payment, current_plan, investor_plan, hodler_plan, referrer, current_user)
        @payment = payment
        @current_plan = current_plan
        @investor_plan = investor_plan
        @hodler_plan = hodler_plan
        @referrer = referrer
        @current_user = current_user
      end

      def current_plan_name
        @current_plan_name ||= current_plan.name
      end

      def available_plans
        @available_plans ||= case current_plan.name
                             when 'saver' then %w[investor hodler]
                             when 'investor' then available_plans_for_investor
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

      private

      def available_plans_for_investor
        return %w[investor hodler] if @current_user.subscription.end_time <= Time.current + 1.years

        %w[hodler]
      end
    end
  end
end
