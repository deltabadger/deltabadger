module Payments
  class Client < BaseService
    include Rails.application.routes.url_helpers

    HOST                  = ENV.fetch('PAYMENTS_CALLBACK_HOST')
    URL                   = ENV.fetch('PAYMENTS_URL')
    PUBLIC_API_KEY        = ENV.fetch('PAYMENTS_API_KEY')
    AUTHORIZATION_HEADER  = ENV.fetch('PAYMENTS_AUTHORIZATION_HEADER')

    def initialize(
      api_key: PUBLIC_API_KEY,
      authorization_header: AUTHORIZATION_HEADER
    )
      @api_key = api_key
      @authorization_header = authorization_header
    end

    def create_payment(params)
      try_create_payment(params)
    rescue Faraday::ClientError => e
      Result::Failure.new(e.message)
    rescue StandardError => e
      Raven.capture_exception(e) # we do not anticipate standard errors
      Result::Failure.new(e.message)
    end

    private

    attr_reader :api_key, :authorization_header

    def try_create_payment(params)
      url = create_url('invoices/')

      response = Faraday.post(url, body(params), headers)

      return Result::Failure.new(response.body) unless response.success?

      response = JSON.parse(response.body)

      if response['error']
        Result::Failure.new(response['error'])
      else
        data = response.fetch('data')
        Result::Success.new(result_data(data))
      end
    end

    def create_url(endpoint)
      "#{URL}#{endpoint}"
    end

    def body(params)
      price = params.fetch(:price)
      currency = params.fetch(:currency)
      email = params.fetch(:email)

      {
        price: price,
        currency: currency,
        buyer: { email: email },
        redirectUrl: upgrade_payment_success_url(host: HOST),
        notificationUrl: upgrade_payment_callback_url(host: HOST),
        extendedNotifications: true,
        token: api_key
      }.to_json
    end

    def headers
      {
        'x-accept-version': '2.0.0',
        'Accept' => 'application/json',
        'Content-Type' => 'application/json',
        'Authorization' => authorization_header
      }
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
