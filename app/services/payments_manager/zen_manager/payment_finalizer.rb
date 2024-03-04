module PaymentsManager
  module ZenManager
    class PaymentFinalizer < BaseService
      PAID_STATUSES = %w[ACCEPTED].freeze
      # CANCELLED_STATUSES = %i[expired invalid].freeze

      def initialize
        @notifications = Notifications::Subscription.new
        @fomo_notifications = Notifications::FomoEvents.new
        @subscribe_plan = SubscribePlan.new
        @grant_commission = Affiliates::GrantCommission.new
      end

      def call(params)
        return Result::Failure.new unless params[:status].in(PAID_STATUSES)

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
        @subscribe_plan.call(
          user: payment.user,
          subscription_plan: payment.subscription_plan,
          email_params: nil
        )

        @grant_commission.call(referee: payment.user, payment: payment)

        @fomo_notifications.plan_bought(
          first_name: payment.first_name,
          country: payment.country,
          plan_name: payment.subscription_plan.name
        )

        Result::Success.new
      end
    end
  end
end
