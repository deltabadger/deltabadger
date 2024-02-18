module PaymentsManager
  module ZenManager
    class ZenClient < ApplicationService
      include Rails.application.routes.url_helpers

      HOST                = ENV.fetch('MAIN_PAGE_URL').freeze
      URL                 = ENV.fetch('ZEN_CHECKOUT_URL').freeze
      ZEN_TERMINAL_UUID   = ENV.fetch('ZEN_TERMINAL_UUID').freeze
      ZEN_PAYWALL_SECRET  = ENV.fetch('ZEN_PAYWALL_SECRET').freeze
      ZEN_IPN_SECRET      = ENV.fetch('ZEN_IPN_SECRET').freeze

      def create_payment(params)
        try_create_payment(params)
      rescue Faraday::ClientError => e
        Result::Failure.new(e.message)
      rescue StandardError => e
        Raven.capture_exception(e) # we do not anticipate standard errors
        Result::Failure.new(e.message)
      end

      def get_ipn_hash(params)
        # get value merchantTransactionId from params
        query_string = [
          params[:merchantTransactionId],
          params[:currency],
          params[:amount],
          params[:transactionStatus],
          ZEN_IPN_SECRET
        ].join
        Digest::SHA256.hexdigest(query_string).upcase
      end

      private

      def try_create_payment(params)
        conn = Faraday.new(url: URL)

        response = conn.post do |req|
          req.url '/api/checkouts'
          req.headers['Content-Type'] = 'application/json'
          req.body = create_request_body(params)
        end

        return Result::Failure.new(response.body) unless response.success?

        response = JSON.parse(response.body)
        puts "response: #{response.inspect}"

        if response['error']
          Result::Failure.new(response['error'])
        else
          Result::Success.new(result_data(response))
        end
      end

      def create_request_body(params)
        price = (params.fetch(:price).to_f / 10).round(2).to_s
        request_body = {
          terminalUuid: ZEN_TERMINAL_UUID,
          amount: price,
          currency: params.fetch(:currency),
          merchantTransactionId: params.fetch(:order_id).to_s,
          customer: {
            firstName: params.fetch(:first_name),
            lastName: params.fetch(:last_name),
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
          specifiedPaymentMethod: 'PME_CARD',
          # specifiedPaymentChannel: 'PCL_CARD',
          # urlSuccess: upgrade_zen_payment_success_url(host: HOST, lang: I18n.locale),
          # urlFailure: upgrade_zen_payment_failure_url(host: HOST, lang: I18n.locale)
          urlRedirect: upgrade_zen_payment_finished_url(host: HOST, lang: I18n.locale),
          customIpnUrl: upgrade_zen_payment_ipn_url(host: HOST, lang: I18n.locale)
        }
        request_body[:signature] = get_zen_signature(request_body)
        request_body.to_json
      end

      # rubocop:disable Metrics/PerceivedComplexity
      def hash_to_query_string(hash, parent_key = '')
        params = []

        hash.each do |key, value|
          current_key = parent_key.empty? ? key.to_s : "#{parent_key}.#{key}"

          if value.is_a?(Hash)
            params << hash_to_query_string(value, current_key)
          elsif value.is_a?(Array)
            value.each_with_index do |item, index|
              params << if item.is_a?(Hash)
                          hash_to_query_string(item, "#{current_key}[#{index}]")
                        else
                          "#{current_key}[#{index}]=#{item}"
                        end
            end
          else
            params << "#{current_key}=#{value}"
          end
        end

        params.join('&')
      end
      # rubocop:enable Metrics/PerceivedComplexity

      def get_zen_signature(hash)
        query_string = hash_to_query_string(hash).downcase.split('&').sort.join('&') + ZEN_PAYWALL_SECRET
        "#{Digest::SHA256.hexdigest(query_string)};sha256"
      end

      def result_data(data)
        {
          payment_url: data.fetch('redirectUrl')
        }
      end
    end
  end
end
