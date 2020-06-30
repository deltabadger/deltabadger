module ExchangeApi
  module Clients
    class Bitclude < ExchangeApi::Clients::Base
      URL = 'https://api.bitclude.com/'.freeze
      KEY_VALID_NO_CANCELLABLE_TRANSACTION_CODE = 5057
      MIN_TRANSACTION_VOLUME = 0.0005

      def initialize(api_key:, api_secret:, map_errors: ExchangeApi::MapErrors::Bitclude.new)
        @api_key = api_key
        @api_secret = api_secret
        @map_errors = map_errors
      end

      def current_bid_ask_price(currency)
        url = URL + 'stats/ticker.json'
        request = public_get(url, method: 'account', action: 'info')
        response = JSON.parse(request.body)
        rates = response.fetch("btc_#{currency.downcase}")
        bid = rates['bid'].to_f
        ask = rates['ask'].to_f
        Result::Success.new(BidAskPrice.new(bid, ask))
      rescue StandardError
        Result::Failure.new('Could not fetch current price from BitClude', RECOVERABLE)
      end

      def validate_credentials
        request = private_get(URL, method: 'transactions', action: 'canceloffers', bid: [], ask: [])
        response = JSON.parse(request.body)
        response['code'] == KEY_VALID_NO_CANCELLABLE_TRANSACTION_CODE
      rescue StandardError
        false
      end

      def buy(currency:, price:)
        puts 'Buying on BitClude'
        try_make_order('buy', currency, price)
      end

      def sell(currency:, price:)
        puts 'Selling on BitClude'
        try_make_order('sell', currency, price)
      end

      private

      attr_reader :api_key, :api_secret

      def try_make_order(offer_type, currency, price)
        make_order(offer_type, currency, price)
      rescue StandardError
        Result::Failure.new('Could not make BitClude order', RECOVERABLE)
      end

      def make_order(offer_type, currency, price)
        currency = currency.downcase

        rate_volume_result = smart_rate_volume(offer_type, currency, price)
        return rate_volume_result unless rate_volume_result.success?

        rate, volume = rate_volume_result.data

        request = private_get(
          URL,
          method: 'transactions',
          action: offer_type,
          market1: 'btc',
          market2: currency,
          rate: rate,
          amount: volume
        )

        response = JSON.parse(request.body)

        return error_to_failure([response.fetch('message')]) if response['success'] != true

        Result::Success.new(
          offer_id: response.dig('actions', offer_type).first,
          rate: rate,
          amount: volume
        )
      end

      def public_get(url, params = {})
        Faraday.get(url, params, {})
      end

      def private_get(url, params = {})
        params = { id: api_key, key: api_secret }.merge(params)
        Faraday.get(url, params, {})
      end

      def smart_rate_volume(offer_type, currency, price)
        rate = if offer_type == 'sell'
                 current_bid_price(currency)
               else
                 current_ask_price(currency)
               end
        return rate unless rate.success?

        volume = (price / rate.data).ceil(8)

        Result::Success.new([rate.data, [MIN_TRANSACTION_VOLUME, volume].max])
      end
    end
  end
end
