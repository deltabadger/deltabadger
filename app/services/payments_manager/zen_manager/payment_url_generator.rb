module PaymentsManager
  module ZenManager
    class PaymentUrlGenerator < BaseService
      include Rails.application.routes.url_helpers

      HOST                = ENV.fetch('MAIN_PAGE_URL').freeze
      ZEN_TERMINAL_UUID   = ENV.fetch('ZEN_TERMINAL_UUID').freeze

      def initialize
        @client = ZenClient.new
      end

      def call(payment, user)
        hash = build_request_body(payment, user)
        response = @client.checkout(hash)
        return response if response.failure?

        Result::Success.new(format_checkout_output(response.data))
      end

      private

      def build_request_body(payment, user)
        price = format('%0.02f', payment.total)
        {
          terminalUuid: ZEN_TERMINAL_UUID,
          amount: price,
          currency: payment.currency,
          merchantTransactionId: payment.id.to_s,
          customer: {
            # firstName: user.first_name,
            # lastName: user.last_name,
            email: user.email
          },
          items: [
            {
              name: get_item_description(payment),
              price: price,
              quantity: 1,
              lineAmountTotal: price
            }
          ],
          billingAddress: {
            countryState: payment.country
          },
          # specifiedPaymentMethod: 'PME_CARD',
          # specifiedPaymentChannel: 'PCL_CARD',
          urlRedirect: upgrade_zen_payment_finished_url(host: HOST, lang: I18n.locale),
          customIpnUrl: upgrade_zen_payment_ipn_url(host: HOST, lang: I18n.locale)
        }
      end

      def format_checkout_output(output)
        {
          payment_url: output.fetch('redirectUrl')
        }
      end

      def get_item_description(payment)
        "#{SubscriptionPlan.find(payment.subscription_plan_id).name.capitalize} Plan Upgrade"
      end
    end
  end
end
