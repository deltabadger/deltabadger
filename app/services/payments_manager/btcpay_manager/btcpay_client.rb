module PaymentsManager
  module BtcpayManager
    class BtcpayClient < BaseService
      include Rails.application.routes.url_helpers

      HOST                  = ENV.fetch('MAIN_PAGE_URL').freeze
      URL                   = ENV.fetch('BTCPAY_SERVER_URL').freeze
      API_KEY               = ENV.fetch('BTCPAY_API_KEY').freeze
      AUTHORIZATION_HEADER  = ENV.fetch('BTCPAY_AUTHORIZATION_HEADER').freeze

      def create_payment(params)
        try_create_payment(params)
      rescue Faraday::ClientError => e
        Result::Failure.new(e.message)
      rescue StandardError => e
        Raven.capture_exception(e) # we do not anticipate standard errors
        Result::Failure.new(e.message)
      end

      private

      def try_create_payment(params)
        conn = Faraday.new(url: URL)

        response = conn.post do |req|
          req.url '/invoices/'
          req.headers = {
            'x-accept-version' => '2.0.0',
            'Accept' => 'application/json',
            'Content-Type' => 'application/json',
            'Authorization' => AUTHORIZATION_HEADER
          }
          req.body = create_request_body(params)
        end

        return Result::Failure.new(response.body) unless response.success?

        response = JSON.parse(response.body)

        if response['error']
          Result::Failure.new(response['error'])
        else
          data = response.fetch('data')
          Result::Success.new(result_data(data))
        end
      end

      def create_request_body(params)
        price = params.fetch(:price)
        currency = params.fetch(:currency)
        email = params.fetch(:email)
        order_id = params.fetch(:order_id)
        name = params.fetch(:name)
        birth_date = params.fetch(:birth_date)
        country = params.fetch(:country)
        item_description = params.fetch(:item_description)
        {
          price: price,
          currency: currency,
          orderId: order_id,
          buyer: { email: email,
                   name: name,
                   # It is passed as phone because BTCPay server doesn't accept birth date and we don't need a phone anyway
                   phone: birth_date,
                   country: country },
          itemDesc: item_description,
          redirectUrl: upgrade_btcpay_payment_success_url(host: HOST, lang: I18n.locale),
          notificationUrl: upgrade_btcpay_payment_callback_url(host: HOST, lang: I18n.locale),
          extendedNotifications: true,
          token: API_KEY
        }.to_json
      end

      def result_data(data)
        {
          payment_id: data.fetch('id'),
          status: 'unpaid',
          external_statuses: data.fetch('status'),
          total: data.fetch('price'),
          crypto_total: data.fetch('btcPrice'),
          payment_url: data.fetch('url')
        }
      end
    end
  end
end
