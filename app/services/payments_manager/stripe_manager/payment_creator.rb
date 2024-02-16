module PaymentsManager
  module StripeManager
    class PaymentCreator < ApplicationService

      def initialize(params, discounted)
        @params = params
        @discounted = discounted
        @payments_repository = PaymentsRepository.new
      end

      def call
        cost_calculator = get_stripe_cost_calculator(@params)
        total = cost_calculator.total_price
        # TODO: change to currency(payment)
        currency = @params['country'] == 'Other' ? 0 : 1 # 0 is for USD and 1 is for EUR. All people outside Europe get their prices in USD
        @payments_repository.create(
          status: :paid,
          user: User.find(@params[:user_id]),
          country: @params[:country],
          subscription_plan_id: @params[:subscription_plan_id],
          birth_date: Time.now.strftime('%d/%m/%Y'),
          discounted: @discounted,
          total: total,
          currency: currency,
          commission: cost_calculator.commission,
          payment_type: 'stripe',
          paid_at: Time.now.strftime('%d/%m/%Y')
        )
      end

      private

      def get_stripe_cost_calculator(params)
        subscription_plan = SubscriptionPlan.find(params[:subscription_plan_id])
        plan_name = subscription_plan.name.to_sym
        country_presenter = params[:cost_presenters][params[:country]]
        country_presenter[plan_name].cost_calculator
      end
    end
  end
end
