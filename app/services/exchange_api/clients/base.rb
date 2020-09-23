require 'result'

module ExchangeApi
  module Clients
    class Base
      RECOVERABLE = { data: { recoverable: true }.freeze }.freeze

      def current_bid_ask_price(_currency)
        raise NotImplementedError
      end

      def validate_credentials
        raise NotImplementedError
      end

      def market_buy
        raise NotImplementedError
      end

      def market_sell
        raise NotImplementedError
      end

      def limit_buy
        raise NotImplementedError
      end

      def limit_sell
        raise NotImplementedError
      end

      def current_price(currency)
        result = current_bid_ask_price(currency)
        return result unless result.success?

        price = result.data
        Result::Success.new((price.bid + price.ask) / 2)
      end

      def current_bid_price(currency)
        result = current_bid_ask_price(currency)
        return result unless result.success?

        Result::Success.new(result.data.bid)
      end

      def current_ask_price(currency)
        result = current_bid_ask_price(currency)
        return result unless result.success?

        Result::Success.new(result.data.ask)
      end

      def current_offer_price(offer_type, currency)
        rate = offer_type == 'sell' ? current_bid_price(currency) : current_ask_price(currency)
        return rate unless rate.success?

        Result::Success.new(rate.data)
      end

      def limit_rate(offer_type, currency, percentage)
        percentage = -percentage if offer_type == 'buy'
        offer_price = current_offer_price(offer_type, currency)
        return offer_price unless offer_price.success?

        Result::Success.new(offer_price.data * (1 + percentage / 100))
      end

      protected

      def error_to_failure(error)
        mapped_error = @map_errors.call(error)
        Result::Failure.new(
          *mapped_error.message, data: { recoverable: mapped_error.recoverable }
        )
      end
    end
  end
end
