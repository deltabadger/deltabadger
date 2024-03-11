module PaymentsManager
  module BtcpayManager
    class InvoiceCreator < BaseService
      include Rails.application.routes.url_helpers

      HOST = ENV.fetch('MAIN_PAGE_URL').freeze

      def initialize
        @client = BtcpayClient.new
      end

      def call(payment, user)
        hash = build_request_body(payment, user)
        invoice_result = @client.invoice(hash)
        return invoice_result if invoice_result.failure?
        return Result::Failure.new(invoice_result.data['error']) if invoice_result.data['error']

        Result::Success.new(format_invoice_output(invoice_result.data))
      end

      private

      def build_request_body(payment, user)
        {
          price: payment.total.to_s,
          currency: payment.currency,
          orderId: payment.id,
          buyer: { email: user.email,
                   name: "#{payment.first_name} #{payment.last_name}",
                   # It is passed as phone because BTCPay server doesn't accept birth date and we don't need a phone anyway
                   phone: payment.birth_date,
                   country: payment.country },
          itemDesc: get_item_description(payment),
          redirectUrl: upgrade_btcpay_payment_success_url(host: HOST, lang: I18n.locale),
          notificationUrl: upgrade_btcpay_payment_ipn_url(host: HOST, lang: I18n.locale),
          extendedNotifications: true
        }
      end

      def format_invoice_output(output)
        data = output.fetch('data')
        {
          payment_id: data.fetch('id'),
          status: 'unpaid',
          external_statuses: data.fetch('status'),
          total: data.fetch('price'),
          crypto_total: data.fetch('btcPrice'),
          payment_url: data.fetch('url')
        }
      end

      def get_item_description(payment)
        "#{SubscriptionPlan.find(payment.subscription_plan_id).name.capitalize} Plan Upgrade"
      end
    end
  end
end
