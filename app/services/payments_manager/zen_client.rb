module PaymentsManager
  class ZenClient < BaseService

    ZEN_TERMINAL_UUID = ENV.fetch('ZEN_TERMINAL_UUID').freeze
    ZEN_PAYWALL_SECRET = ENV.fetch('ZEN_PAYWALL_SECRET').freeze

  def test
    conn = Faraday.new(url: 'https://secure.zen.com')

    response = conn.post do |req|
      req.url '/api/checkouts'
      req.headers['Content-Type'] = 'application/json'
      temp_body = {
        terminalUuid: ZEN_TERMINAL_UUID,
        amount: 49,
        currency: 'EUR',
        merchantTransactionId: 'zen_trasaction_1',
        customer: {
          email: current_user.email
        },
        items: [
          {
            name: 'investor_plan',
            price: 49,
            quantity: 1,
            lineAmountTotal: 49
          }
        ],
        billingAddress: {
          countryState: ''
        },
        # urlSuccess: '#',
        # urlFailure: '#'
      }
      temp_body[:signature] = get_zen_signature(temp_body)
      req.body = temp_body.to_json
    end
    puts JSON.parse(response.body)
    @zen_checkout_link = JSON.parse(response.body)['redirectUrl']
  end

  private

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
end