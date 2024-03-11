require 'utilities/number'

module PaymentsManager
  module BtcpayManager
    class PaymentFinalizer < BaseService
      PAID_STATUSES = %i[paid confirmed complete].freeze
      CANCELLED_STATUSES = %i[expired invalid].freeze

      def initialize
        @notifications = Notifications::Subscription.new
        @grant_commission = Affiliates::GrantCommission.new
      end

      def call(invoice)
        params = invoice['data']
        payment = Payment.find_by(payment_id: params['id'])
        external_status = params['status'].to_sym
        status = internal_status(external_status)
        just_paid = just_paid?(payment, status)

        update_params = {
          external_statuses: new_external_statuses(payment, external_status),
          crypto_paid: params['btcPaid']
        }

        update_params.merge!(status: status) unless payment.paid?
        if just_paid
          update_params.merge!(paid_at: paid_at(params),
                               commission: recalculate_commission(params, payment),
                               crypto_commission: recalculate_crypto_commission(params, payment))
        end

        unless payment.update(update_params)
          return Result::Failure.new(payment.errors.full_messages.join(', '), data: update_params)
        end

        return Result::Failure.new('Still not paid') unless just_paid

        @notifications.invoice(payment: payment)
        @grant_commission.call(referee: payment.user, payment: payment)

        PaymentsManager::SubscriptionUpgrader.call(payment.id)
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

      def recalculate_crypto_commission(params, payment)
        return 0 if Utilities::Number.to_bigdecimal(payment.crypto_total, precision: 8).zero?

        params['btcPaid'].to_f / payment.crypto_total * payment.crypto_commission
      end

      def recalculate_commission(params, payment)
        return 0 if Utilities::Number.to_bigdecimal(payment.crypto_total, precision: 8).zero?

        (params['btcPaid'].to_f / payment.crypto_total * payment.commission.to_f).round(2)
      end
    end
  end
end
