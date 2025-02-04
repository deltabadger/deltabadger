module PaymentsManager
  module BtcpayManager
    class PaymentFinalizer < BaseService
      PAID_STATUSES = %i[paid confirmed complete].freeze
      CANCELLED_STATUSES = %i[expired invalid].freeze

      def initialize
        @notifications = Notifications::Subscription.new
      end

      def call(invoice)
        params = invoice['data']
        payment = Payment.find_by(payment_id: params['id'])
        external_status = params['status'].to_sym
        status = internal_status(external_status)
        just_paid = just_paid?(payment, status)

        update_params = {
          external_statuses: new_external_statuses(payment, external_status),
          btc_paid: params['btcPaid']
        }
        update_params.merge!(status: status) unless payment.paid?
        update_params.merge!(paid_at: paid_at(params)) if just_paid
        return Result::Failure.new('ActiveRecord error', data: update_params) unless payment.update(update_params)

        return Result::Failure.new('Still not paid') unless just_paid

        @notifications.invoice(payment: payment)

        GrantAffiliateCommissionJob.perform_later(payment.id)

        PaymentsManager::SubscriptionUpgrader.call(payment)
      end

      private

      def internal_status(external_status)
        if external_status.in?(PAID_STATUSES)
          :paid
        elsif external_status.in?(CANCELLED_STATUSES)
          :cancelled
        else
          :unpaid
        end
      end

      def just_paid?(payment, status)
        !payment.paid? && status == :paid
      end

      def new_external_statuses(payment, external_status)
        if payment.external_statuses.empty?
          external_status
        else
          "#{payment.external_statuses}, #{external_status}"
        end
      end

      def paid_at(params)
        Time.at(params['currentTime'] / 1000)
      end
    end
  end
end
