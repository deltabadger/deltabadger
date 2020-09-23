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
          Result::Failure.new('Something went wrong!', RECOVERABLE)
        end
      end

      def market_buy(currency:, price:)
        puts "Fake: Market buying things on #{exchange_name}!"
        make_order('buy', currency, price)
      end

      def market_sell(currency:, price:)
        puts "Fake: Market selling things on #{exchange_name}!"
        make_order('sell', currency, price)
      end

      def limit_buy(currency:, price:, percentage:)
        puts "Fake: Limit buying things on #{exchange_name}!"
        make_order('buy', currency, price, percentage)
      end

      def limit_sell(currency:, price:, percentage:)
        puts "Fake: Limit selling things on #{exchange_name}!"
        make_order('sell', currency, price, percentage)
      end

      private

      def make_order(offer_type, currency, price, percentage = 0)
        volume_result = smart_volume(offer_type, currency, price, percentage)
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
      rescue StandardError
        Result::Failure.new('Caught an error while making fake order', RECOVERABLE)
      end

      def smart_volume(offer_type, currency, price, percentage)
        rate = limit_rate(offer_type, currency, percentage)
        return rate unless rate.success?

        volume = (price / rate.data).ceil(8)
        Result::Success.new([MIN_TRANSACTION_VOLUME, volume].max)
      end

      def new_prices
        @bid = rand(6000...8000)
        @ask = @bid * (1 + rand * 0.2)
      end
    end
  end
end
