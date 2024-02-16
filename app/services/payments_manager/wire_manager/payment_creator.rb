module PaymentsManager
  module WireManager
    class PaymentCreator < ApplicationService
      def initialize(params, discounted)
        @params = params
        @discounted = discounted
        @payments_repository = PaymentsRepository.new
      end

      def call
        cost_calculator = get_wire_cost_calculator(@params)
        total = cost_calculator.total_price
        # TODO: change to currency(payment)
        currency = @params['country'] == 'Other' ? 0 : 1 # 0 is for USD and 1 is for EUR. All people outside Europe get their prices in USD
        @payments_repository.create(
          id: PaymentsManager::NextPaymentIdGetter.call,
          status: :pending,
          user: @params[:user],
          first_name: @params[:first_name],
          last_name: @params[:last_name],
          country: @params[:country],
          subscription_plan_id: @params[:subscription_plan_id],
          birth_date: Time.now.strftime('%d/%m/%Y'),
          discounted: @discounted,
          payment_type: 'wire',
          total: total,
          currency: currency
        )
      end

      private

      def get_wire_cost_calculator(params)
        params['cost_presenters'][params['country']][SubscriptionPlan.find(params['subscription_plan_id']).name].cost_calculator
      end
    end
  end
end
