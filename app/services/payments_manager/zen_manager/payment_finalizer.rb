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
        unless payment.update(update_params)
          return Result::Failure.new(payment.errors.full_messages.join(', '), data: update_params)
        end

        @notifications.invoice(payment: payment)

        PaymentsManager::SubscriptionUpgrader.call(payment.id)
      end
    end
  end
end
