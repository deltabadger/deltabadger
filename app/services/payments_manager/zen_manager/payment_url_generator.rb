module PaymentsManager
  module ZenManager
    class PaymentUrlGenerator < BaseService
      include Rails.application.routes.url_helpers

      HOST                = ENV.fetch('MAIN_PAGE_URL').freeze
      ZEN_TERMINAL_UUID   = ENV.fetch('ZEN_TERMINAL_UUID').freeze

      def initialize
        @client = ZenClient.new
      end

      def call(params)
        hash = build_request_body(params)
        response = @client.checkout(hash)
        return response if response.failure?

        Result::Success.new(format_checkout_output(response.data))
      end

      private

      def build_request_body(params)
        price = format_price(params.fetch(:price))
        {
          terminalUuid: ZEN_TERMINAL_UUID,
          amount: price,
          currency: params.fetch(:currency),
          merchantTransactionId: params.fetch(:order_id).to_s,
          customer: {
            # firstName: params.fetch(:first_name),
            # lastName: params.fetch(:last_name),
            email: params.fetch(:email)
          },
          items: [
            {
              name: params.fetch(:item_description),
              price: price,
              quantity: 1,
              lineAmountTotal: price
            }
          ],
          billingAddress: {
            countryState: params.fetch(:country)
          },
          # specifiedPaymentMethod: 'PME_CARD',
          # specifiedPaymentChannel: 'PCL_CARD',
          urlRedirect: upgrade_zen_payment_finished_url(host: HOST, lang: I18n.locale),
          customIpnUrl: upgrade_zen_payment_ipn_url(host: HOST, lang: I18n.locale)
        }
      end

      def format_price(price)
        format('%0.02f', price)
      end

      def format_checkout_output(output)
        {
          payment_url: output.fetch('redirectUrl')
        }
      end
    end
  end
end
