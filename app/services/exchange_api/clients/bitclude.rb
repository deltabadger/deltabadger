module ExchangeApi
  module Clients
    class Bitclude < ExchangeApi::Clients::Base
      URL = 'https://api.bitclude.com/'.freeze
      KEY_VALID_NO_CANCELLABLE_TRANSACTION_CODE = 5057
      MIN_TRANSACTION_VOLUME = 0.0005

      def initialize(api_key:, api_secret:)
        @api_key = api_key
        @api_secret = api_secret
      end

      def current_bid_ask_price(settings)
        url = URL + 'stats/ticker.json'
        request = public_get(url, method: 'account', action: 'info')
        response = JSON.parse(request.body)
        rates = response.fetch("btc_#{settings.fetch('currency').downcase}")
        bid = rates['bid'].to_f
        ask = rates['ask'].to_f
        Result::Success.new(BidAskPrice.new(bid, ask))
      rescue StandardError => e
        Result::Failure.new('Could not fetch current price from BitClude', e.message)
      end

      def validate_credentials
        request = private_get(URL, method: 'transactions', action: 'canceloffers', bid: [], ask: [])
        response = JSON.parse(request.body)
        response['code'] == KEY_VALID_NO_CANCELLABLE_TRANSACTION_CODE
      rescue StandardError
        false
      end

      def buy(settings)
        puts 'Buying on BitClude'
        make_order('buy', settings)
      end

      def sell(settings)
        puts 'Selling on BitClude'
        make_order('sell', settings)
      end

      private

      attr_reader :api_key, :api_secret

      def try_make_order(offer_type, settings)
        make_order(offer_type, settings)
      rescue StandardError => e
        Result::Failure.new('Could not make BitClude order', e.message)
      end

      def make_order(offer_type, settings)
        currency = settings.fetch('currency').downcase

        rate_volume_result = smart_rate_volume(offer_type, settings)
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

        return Result::Failure.new(response.fetch('message')) if response['success'] != true

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

      def smart_rate_volume(offer_type, settings)
        rate = if offer_type == 'sell'
                 current_bid_price(settings)
               else
                 current_ask_price(settings)
               end
        return rate unless rate.success?

        price = settings.fetch('price').to_f
        volume = (price / rate.data).ceil(8)

        Result::Success.new([rate.data, [MIN_TRANSACTION_VOLUME, volume].max])
      end
    end
  end
end
