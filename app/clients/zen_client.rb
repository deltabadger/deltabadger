class ZenClient < ApplicationClient
  URL                 = ENV.fetch('ZEN_CHECKOUT_URL').freeze
  ZEN_PAYWALL_SECRET  = ENV.fetch('ZEN_PAYWALL_SECRET').freeze

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: true, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # @param hash [Hash] A hash of arguments as described in the checkout integration
  #   documentation at https://www.zen.com/developer/checkout-integration/
  #   Signature is generated within the method and not required in the input.
  #
  # @returns
  #   #=> {"redirectUrl"=>"https://secure.zen.com/4312a1c3-1a54-4e1f-b37c-0e2242986ce1"}
  def checkout(hash = {})
    with_rescue do
      signed_body = hash.merge(signature: get_signature(hash))
      response = self.class.connection.post do |req|
        req.url '/api/checkouts'
        req.body = signed_body
      end
      Result::Success.new(response.body)
    end
  end

  private

  def get_signature(hash_to_sign)
    array_of_strings = get_array_of_strings_from_hash(hash_to_sign)
    string_to_hash = array_of_strings.sort.join('&') + ZEN_PAYWALL_SECRET
    hashed_string = Digest::SHA256.hexdigest(string_to_hash)
    "#{hashed_string};sha256"
  end

  # rubocop:disable Metrics/PerceivedComplexity
  def get_array_of_strings_from_hash(hash, parent_key = '', strings = [])
    hash.each do |key, value|
      current_key = parent_key.empty? ? key.to_s : "#{parent_key}.#{key}"

      if value.is_a?(Hash)
        get_array_of_strings_from_hash(value, current_key, strings)
      elsif value.is_a?(Array)
        value.each_with_index do |item, index|
          if item.is_a?(Hash)
            get_array_of_strings_from_hash(item, "#{current_key}[#{index}]", strings)
          else
            strings << "#{current_key}[#{index}]=#{item}".downcase
          end
        end
      else
        strings << "#{current_key}=#{value}".downcase
      end
    end

    strings
  end
  # rubocop:enable Metrics/PerceivedComplexity
end
