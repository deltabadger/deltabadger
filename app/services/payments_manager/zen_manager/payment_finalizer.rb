module PaymentsManager
  module ZenManager
    class PaymentFinalizer < BaseService
      PAID_STATUSES = %w[ACCEPTED].freeze

      def initialize
        @notifications = Notifications::Subscription.new
      end

      def call(params)
        return Result::Failure.new('Still not paid') unless params[:status].in?(PAID_STATUSES)

        payment = Payment.find(params[:merchantTransactionId])
        Rails.logger.info "Payment found: #{payment.inspect}"

        update_params = {
          status: :paid,
          paid_at: Time.current,
          first_name: params[:customer][:firstName],
          last_name: params[:customer][:lastName]
        }
        return Result::Failure.new('ActiveRecord error', data: update_params) unless payment.update(update_params)

        @notifications.invoice(payment: payment)

        GrantAffiliateCommissionJob.perform_later(payment.id)

        PaymentsManager::SubscriptionUpgrader.call(payment)
      end
    end
  end
end
