module PaymentsManager
  module StripeManager
    class PaymentCreator < BaseService

      def initialize
        @payments_repository = PaymentsRepository.new
      end

      def call(params, user, cost_data)
        # TODO: change to currency(payment)
        currency = params[:country] == 'Other' ? 0 : 1 # 0 is for USD and 1 is for EUR. All people outside Europe get their prices in USD
        @payments_repository.create(
          status: :paid,
          user: user,
          country: params[:country],
          subscription_plan_id: params[:subscription_plan_id],
          birth_date: Time.now.strftime('%d/%m/%Y'),
          discounted: cost_data[:discount_percent_amount].to_f.positive?,
          total: cost_data[:total_price],
          currency: currency,
          commission: cost_data[:commission],
          payment_type: 'stripe',
          paid_at: Time.now.strftime('%d/%m/%Y')
        )
      end
    end
  end
end
