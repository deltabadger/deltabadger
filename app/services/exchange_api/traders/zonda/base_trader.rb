require 'result'

# rubocop#disable Style/StringLiterals
module ExchangeApi
  module Traders
    module Zonda
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Zonda

        def initialize(
          api_key:,
          api_secret:,
          market: ExchangeApi::Markets::Zonda::Market.new,
          map_errors: ExchangeApi::MapErrors::Zonda.new
        )
          @api_key = api_key
          @api_secret = api_secret
          @market = market
          @map_errors = map_errors
        end

        def fetch_order_by_id(_order_id, response_params = nil)
          Result::Success.new(response_params)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch order parameters from Zonda')
        end

        private

        def place_order(symbol, params)
          url = "https://api.zonda.exchange/rest/trading/offer/#{symbol}"
          body = params.to_json
          response = JSON.parse(Faraday.post(url, body, headers(@api_key, @api_secret, body)).body)
          parse_response(response)
        rescue StandardError
          Result::Failure.new('Could not make Zonda order', RECOVERABLE)
        end

        def transaction_price(symbol, price, force_smart_intervals, smart_intervals_value, price_in_quote)
          min_price = price_in_quote ? @market.minimum_order_price(symbol) : @market.minimum_order_price(symbol, !price_in_quote)
          return min_price unless min_price.success?

          unless price_in_quote
            minimum_quote_price = @market.minimum_order_price(symbol)
            return minimum_quote_price unless minimum_quote_price.success?

            current_bid_price = @market.current_bid_price(symbol)
            return current_bid_price unless current_bid_price.success?

            quote_minimum = minimum_quote_price.data / current_bid_price.data

            min_price = Result::Success.new(quote_minimum) if quote_minimum > min_price.data
          end
          smart_intervals_value = min_price.data if smart_intervals_value.nil?

          return Result::Success.new([smart_intervals_value, min_price.data].max) if force_smart_intervals

          rate_decimals = @market.base_decimals(symbol)
          return rate_decimals unless rate_decimals.success?

          result = [min_price.data, price].max
          result = result.ceil(rate_decimals.data)
          Result::Success.new(result)
        end

        def transaction_volume(symbol, price, limit_rate, price_in_quote)
          rate_decimals = @market.base_decimals(symbol)
          return rate_decimals unless rate_decimals.success?

          current_market_rate = @market.current_bid_price(symbol)
          return current_market_rate unless current_market_rate.success?

          price *= current_market_rate.data unless price_in_quote
          Result::Success.new((price / limit_rate).ceil(rate_decimals.data))
        end

        def common_order_params
          { postOnly: false, fillOrKill: false }
        end

        def parse_response(response)
          if response.fetch('status') == 'Ok'
            Result::Success.new(
              offer_id: response.fetch('offerId'),
              rate: response.fetch('transactions').first.fetch('rate'),
              amount: response.fetch('transactions').first.fetch('amount')
            )
          else
            error_to_failure(response.fetch('errors'))
          end
        end
      end
    end
  end
end
