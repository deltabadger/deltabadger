module PaymentsManager
  module ZenManager
    class PaymentFinalizer < BaseService
      PAID_STATUSES = %w[ACCEPTED].freeze

      def initialize
        @notifications = Notifications::Subscription.new
      end

      def call(params)
        return Result::Failure.new unless params[:status].in?(PAID_STATUSES)

        payment = Payment.find(params[:merchantTransactionId])
        Rails.logger.info "Payment found: #{payment.inspect}"

        update_params = {
          status: :paid,
          paid_at: Time.current,
          first_name: params[:customer][:firstName],
          last_name: params[:customer][:lastName]
        }

        Rails.logger.info "Updating payment with params: #{update_params.inspect}"
        Rails.logger.info "Payment: #{payment.inspect}"
        return Result::Failure.new unless payment.update(update_params)

        Rails.logger.info "Payment updated: #{payment.inspect}"
        Rails.logger.info "Payment from DB: #{Payment.find(params[:merchantTransactionId]).inspect}"

        @notifications.invoice(payment: payment)

        PaymentsManager::SubscriptionUpgrader.call(payment.id)
      end
    end
  end
end
