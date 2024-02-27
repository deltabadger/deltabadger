module PaymentsManager
  module WireManager
    class PaymentCreator < BaseService
      def initialize
        @payments_repository = PaymentsRepository.new
      end

      def call(params, discounted)
        cost_data = get_cost_data(params)
        # TODO: change to currency(payment)
        currency = params['country'] == 'Other' ? 0 : 1 # 0 is for USD and 1 is for EUR. All people outside Europe get their prices in USD
        @payments_repository.create(
          id: PaymentsManager::NextPaymentIdGetter.call,
          status: :pending,
          user: params[:user],
          first_name: params[:first_name],
          last_name: params[:last_name],
          country: params[:country],
          subscription_plan_id: params[:subscription_plan_id],
          birth_date: Time.now.strftime('%d/%m/%Y'),
          discounted: discounted,
          payment_type: 'wire',
          total: cost_data[:total_price],
          currency: currency
        )
      end

      private

      def get_cost_data(params)
        params['cost_presenters'][params['country']][SubscriptionPlan.find(params['subscription_plan_id']).name].cost_data
      end
    end
  end
end
