class Clients::ZenCheckout < Client
  URL = ENV.fetch('ZEN_CHECKOUT_URL')
  PAYWALL_SECRET = ENV.fetch('ZEN_PAYWALL_SECRET')
  TERMINAL_UUID = ENV.fetch('ZEN_TERMINAL_UUID')

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

  # https://docs.zen.com/payments/checkout-integration/communication-parameters
  # @param terminal_uuid [String] The terminal UUID
  # @param amount [String] The amount of the transaction
  # @param currency [String] Currency in ISO 4217 alphabetic code of the transaction
  #        (it will determine payment methods displayed on the paywall).
  #        Must be written in all capital letters.
  # @param merchant_transaction_id [String] The merchant's unique identifier of the transaction.
  #        No two requests sent by merchant can have same value in this field.
  # @param customer_id [String] The customer's ID in the merchant's system
  # @param customer_first_name [String] The customer's first name
  # @param customer_last_name [String] The customer's last name
  # @param customer_email [String] The customer's email address
  # @param customer_phone [String] The customer's phone number
  # @param item_code [String] The merchant's code for the sold item
  # @param item_category [String] The merchant's category for the sold item
  # @param item_name [String] The name of the sold item
  # @param item_price [String] The unit price of the sold item
  # @param item_quantity [Integer] The quantity of the sold items
  # @param item_line_amount_total [String] The total price of the sold items
  # @param shipping_address_id [String] The customer's shipping address ID
  # @param shipping_address_first_name [String] The customer's shipping address first name
  # @param shipping_address_last_name [String] The customer's shipping address last name
  # @param shipping_address_country [String] The customer's shipping address country
  # @param billing_address_street [String] The customer's billing address street
  # @param billing_address_city [String] The customer's billing address city
  # @param billing_address_country_state [String] The customer's billing address country state
  # @param billing_address_province [String] The customer's billing address province
  # @param billing_address_building_number [String] The customer's billing address building number
  # @param billing_address_room_number [String] The customer's billing address room number
  # @param billing_address_postcode [String] The customer's billing address postcode
  # @param billing_address_company_name [String] The customer's billing address company name
  # @param billing_address_phone [String] The customer's billing address phone
  # @param billing_address_tax_id [String] The customer's billing address tax ID
  # @param recurring_data_payment_type [String] The type of recurring payment (recurring or unscheduled)
  # @param recurring_data_expiry_date [String] The expiry date of the recurring payment (YYYYMMDD)
  #        With no expiration date plans recommendation is to use "99991212"
  # @param recurring_data_frequency [Integer] Indicates minimum number of days between authorization,
  #        limited to 4 characters. Recommendation is to use "1"
  # @param url_redirect [String] The URL to redirect to after the transaction
  #        (used if url_success and url_failure were not specified)
  # @param url_success [String] The URL to redirect to after a successful transaction
  # @param url_failure [String] The URL to redirect to after a failed transaction
  # @param custom_ipn_url [String] The URL to send IPN to
  # @param specified_payment_method [String] The code to limit checkout to one payment method
  # @param specified_payment_channel [String] The code to limit checkout to one payment channel
  #
  # @returns
  #   #=> {"redirectUrl"=>"https://secure.zen.com/4312a1c3-1a54-4e1f-b37c-0e2242986ce1"}
  def checkout(
    amount:,
    currency:,
    merchant_transaction_id:,
    item_name:,
    item_price:,
    item_quantity:,
    item_line_amount_total:,
    customer_id: nil,
    customer_first_name: nil, # Providing this data increases the approval rate for card payments
    customer_last_name: nil, # Providing this data increases the approval rate for card payments
    customer_email: nil, # Providing this data increases the approval rate for card payments
    customer_phone: nil,
    item_code: nil,
    item_category: nil,
    shipping_address_id: nil,
    shipping_address_first_name: nil,
    shipping_address_last_name: nil,
    shipping_address_country: nil,
    shipping_address_street: nil,
    shipping_address_city: nil,
    shipping_address_country_state: nil,
    shipping_address_province: nil,
    shipping_address_building_number: nil,
    shipping_address_room_number: nil,
    shipping_address_postcode: nil,
    shipping_address_company_name: nil,
    shipping_address_phone: nil,
    billing_address_id: nil,
    billing_address_first_name: nil,
    billing_address_last_name: nil,
    billing_address_country: nil,
    billing_address_street: nil,
    billing_address_city: nil,
    billing_address_country_state: nil,
    billing_address_province: nil,
    billing_address_building_number: nil,
    billing_address_room_number: nil,
    billing_address_postcode: nil,
    billing_address_company_name: nil,
    billing_address_phone: nil,
    billing_address_tax_id: nil,
    recurring_data_payment_type: nil,
    recurring_data_expiry_date: nil,
    recurring_data_frequency: nil,
    url_redirect: nil,
    url_success: nil,
    url_failure: nil,
    custom_ipn_url: nil,
    specified_payment_method: nil,
    specified_payment_channel: nil,
    language: nil
  )
    with_rescue do
      response = self.class.connection.post do |req|
        req.url '/api/checkouts'
        req.body = {
          terminalUuid: TERMINAL_UUID,
          amount: amount,
          currency: currency,
          merchantTransactionId: merchant_transaction_id,
          customer: {
            id: customer_id,
            firstName: customer_first_name,
            lastName: customer_last_name,
            email: customer_email,
            phone: customer_phone
          }.compact.presence,
          items: [
            {
              code: item_code,
              category: item_category,
              name: item_name,
              price: item_price,
              quantity: item_quantity,
              lineAmountTotal: item_line_amount_total
            }.compact
          ],
          shippingAddress: {
            id: shipping_address_id,
            firstName: shipping_address_first_name,
            lastName: shipping_address_last_name,
            country: shipping_address_country,
            street: shipping_address_street,
            city: shipping_address_city,
            countryState: shipping_address_country_state,
            province: shipping_address_province,
            buildingNumber: shipping_address_building_number,
            roomNumber: shipping_address_room_number,
            postcode: shipping_address_postcode,
            companyName: shipping_address_company_name,
            phone: shipping_address_phone
          }.compact.presence,
          billingAddress: {
            id: billing_address_id,
            firstName: billing_address_first_name,
            lastName: billing_address_last_name,
            country: billing_address_country,
            street: billing_address_street,
            city: billing_address_city,
            countryState: billing_address_country_state,
            province: billing_address_province,
            buildingNumber: billing_address_building_number,
            roomNumber: billing_address_room_number,
            postcode: billing_address_postcode,
            companyName: billing_address_company_name,
            phone: billing_address_phone,
            taxId: billing_address_tax_id
          }.compact.presence,
          recurringData: {
            paymentType: recurring_data_payment_type,
            expiryDate: recurring_data_expiry_date,
            frequency: recurring_data_frequency
          }.compact.presence,
          urlRedirect: url_redirect,
          urlSuccess: url_success,
          urlFailure: url_failure,
          customIpnUrl: custom_ipn_url,
          specifiedPaymentMethod: specified_payment_method,
          specifiedPaymentChannel: specified_payment_channel,
          language: language
        }.compact
        req.body[:signature] = sha256_signature(req.body)
      end
      Result::Success.new(response.body)
    end
  end

  private

  def sha256_signature(body)
    string_to_hash = hash_to_strings(body).sort.join('&') + PAYWALL_SECRET
    hashed_string = Digest::SHA256.hexdigest(string_to_hash)
    "#{hashed_string};sha256"
  end

  def hash_to_strings(hash, parent_key = '', strings = []) # rubocop:disable Metrics/PerceivedComplexity
    hash.each do |key, value|
      current_key = parent_key.empty? ? key.to_s : "#{parent_key}.#{key}"

      if value.is_a?(Hash)
        hash_to_strings(value, current_key, strings)
      elsif value.is_a?(Array)
        value.each_with_index do |item, index|
          if item.is_a?(Hash)
            hash_to_strings(item, "#{current_key}[#{index}]", strings)
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
end
