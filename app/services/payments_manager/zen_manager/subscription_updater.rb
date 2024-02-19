module PaymentsManager
  module ZenManager
    class SubscriptionUpdater < ApplicationService
      # PAID_STATUSES = %i[paid confirmed complete].freeze
      # CANCELLED_STATUSES = %i[expired invalid].freeze

      def initialize(params)
        @params = params
        @payments_repository = PaymentsRepository.new
        @notifications = Notifications::Subscription.new
        @fomo_notifications = Notifications::FomoEvents.new
        @subscribe_plan = SubscribePlan.new
        @grant_commission = Affiliates::GrantCommission.new
      end

      def call
        payment = @payments_repository.find(@params[:merchantTransactionId])
        Rails.logger.info "Payment found: #{payment.inspect}"

        update_params = {
          status: :paid,
          paid_at: Time.current
        }

        payment = @payments_repository.update(payment.id, update_params)

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
      end

      # private

      # def internal_status(external_status)
      #   if external_status.in?(PAID_STATUSES)
      #     :paid
      #   elsif external_status.in?(CANCELLED_STATUSES)
      #     :cancelled
      #   else
      #     :unpaid
      #   end
      # end

      # def just_paid?(payment, status)
      #   !payment.paid? && status == :paid
      # end

      # def new_external_statuses(payment, external_status)
      #   if payment.external_statuses.empty?
      #     external_status
      #   else
      #     "#{payment.external_statuses}, #{external_status}"
      #   end
      # end

      # def paid_at(params)
      #   Time.at(params['currentTime'] / 1000)
      # end

      # def recalculate_crypto_commission(params, payment)
      #   # TODO: use better match to zero
      #   return 0.0 if payment.crypto_total.to_f <= 0.0

      #   params['btcPaid'].to_f / payment.crypto_total * payment.crypto_commission
      # end

      # def recalculate_commission(params, payment)
      #   # TODO: use better match to zero
      #   return 0.0 if payment.crypto_total.to_f <= 0.0

      #   (params['btcPaid'].to_f / payment.crypto_total * payment.commission.to_f).round(2)
      # end
    end
  end
end
