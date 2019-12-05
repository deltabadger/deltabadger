module Payments
  class Client < BaseService
    URL = ENV.fetch('PAYMENTS_URL', 'https://test.globee.com/payment-api/v1/')
    # URL = ENV.fetch('PAYMENTS_URL', 'https://globee.com/payment-api/v1/')
    PUBLIC_API_KEY = ENV.fetch('PAYMENTS_API_KEY')

    def initialize(api_key: PUBLIC_API_KEY)
      @api_key = api_key
    end

    def ping
      url = create_url('ping')
      JSON.parse(Faraday.get(url, {}, headers).body)
    end

    def create_payment(params)
      url = create_url('payment-request')

      response = JSON.parse(
        Faraday.post(url, body(params), headers).body
      )

      if response['success']
        data = response.fetch('data')

        Result::Success.new(
          id: data.fetch('id'),
          status: data.fetch('status'),
          total: data.fetch('total'),
          email: data.fetch('customer').fetch('email'),
          created_at: Time.parse(data.fetch('created_at')),
          payment_url: data.fetch('redirect_url')
        )
      else
        Result::Failure.new(response.fetch('errors'))
      end
    end

    def get_payment(id)
      url = create_url("payment-request/#{id}")

      response = JSON.parse(
        Faraday.get(url, {}, headers).body
      )
      if response['success']
        data = response.fetch('data')

        Result::Success.new(
          id: data.fetch('id'),
          status: data.fetch('status'),
          total: data.fetch('total'),
          email: data.fetch('customer').fetch('email'),
          created_at: Time.parse(data.fetch('created_at')),
          payment_url: data.fetch('redirect_url')
        )
      else
        Result::Failure.new([response.fetch('message')])
      end
    end

    private

    def body(params)
      price = params.fetch(:price)
      currency = params.fetch(:currency)

      {
        total: price,
        currency: currency,
        callback_data: 'example data',
        customer: { email: 'tomasz.balon@upsidelab.io' }
      }.to_json
    end

    def create_url(endpoint)
      "#{URL}#{endpoint}"
    end

    def headers
      {
        'X-AUTH-KEY' => @api_key,
        'Accept' => 'application/json',
        'Content-Type' => 'application/json'
      }
    end
  end
end
