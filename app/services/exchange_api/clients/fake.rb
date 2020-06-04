module ExchangeApi
  module Clients
    class Fake < ExchangeApi::Clients::Base
      MIN_TRANSACTION_VOLUME = 0.002

      SUCCESS = true
      # SUCCESS = false

      attr_reader :exchange_name, :bid, :ask

      def initialize(exchange_name)
        @exchange_name = exchange_name
        new_prices
      end

      def validate_credentials
        SUCCESS
      end

      def current_bid_ask_price(_)
        if SUCCESS
          new_prices
          Result::Success.new(BidAskPrice.new(bid, ask))
        else
          Result::Failure.new('Something went wrong!')
        end
      end

      def buy(settings)
        puts "Fake: Buying things on #{exchange_name}!"
        make_order('buy', settings)
      end

      def sell(settings)
        puts "Fake: Selling things on #{exchange_name}!"
        make_order('sell', settings)
      end

      private

      def make_order(offer_type, settings)
        volume_result = smart_volume(offer_type, settings)
        return volume_result unless volume_result.success?

        volume = volume_result.data

        if SUCCESS
          Result::Success.new(
            offer_id: SecureRandom.uuid,
            rate: offer_type == 'sell' ? bid : ask,
            amount: volume
          )
        else
          Result::Failure.new('Something went wrong!')
        end
      rescue StandardError => e
        Result::Failure.new('Caught an error while making fake order', e.message)
      end

      def smart_volume(offer_type, settings)
        rate = if offer_type == 'sell'
                 current_bid_price(settings)
               else
                 current_ask_price(settings)
               end
        return rate unless rate.success?

        price = settings.fetch('price').to_f
        volume = price / rate.data

        Result::Success.new([MIN_TRANSACTION_VOLUME, volume].max)
      end

      def new_prices
        @bid = rand(6000...8000)
        @ask = @bid * (1 + rand * 0.2)
      end
    end
  end
end
