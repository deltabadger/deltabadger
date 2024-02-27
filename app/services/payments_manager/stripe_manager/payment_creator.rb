module PaymentsManager
  module StripeManager
    class PaymentCreator < BaseService

      def initialize
        @payments_repository = PaymentsRepository.new
      end

      def call(params, discounted)
        cost_data = get_cost_data(params)
        # TODO: change to currency(payment)
        currency = params['country'] == 'Other' ? 0 : 1 # 0 is for USD and 1 is for EUR. All people outside Europe get their prices in USD
        @payments_repository.create(
          status: :paid,
          user: User.find(params[:user_id]),
          country: params[:country],
          subscription_plan_id: params[:subscription_plan_id],
          birth_date: Time.now.strftime('%d/%m/%Y'),
          discounted: discounted,
          total: cost_data[:total_price],
          currency: currency,
          commission: cost_data[:commission],
          payment_type: 'stripe',
          paid_at: Time.now.strftime('%d/%m/%Y')
        )
      end

      private

      def get_cost_data(params)
        subscription_plan = SubscriptionPlan.find(params[:subscription_plan_id])
        plan_name = subscription_plan.name.to_sym
        country_presenter = params[:cost_presenters][params[:country]]
        country_presenter[plan_name].cost_data
      end
    end
  end
end
