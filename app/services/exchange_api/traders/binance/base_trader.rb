require 'result'

module ExchangeApi
  module Traders
    module Binance
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Binance

        def initialize(
          api_key:,
          api_secret:,
          url_base:,
          market: ExchangeApi::Markets::Binance::Market,
          map_errors: ExchangeApi::MapErrors::Binance.new
        )
          @signed_client = signed_client(api_key, api_secret, url_base)
          @market = market.new(url_base: url_base)
          @map_errors = map_errors
        end

        def fetch_order_by_id(_order_id, response_params = nil)
          Result::Success.new(response_params)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch order parameters from Binance')
        end

        private

        def place_order(order_params)
          request = @signed_client.post('order') do |req|
            req.params = order_params
          end

          response = JSON.parse(request.body)
          parse_response(response)
        rescue StandardError
          Result::Failure.new('Could not make Binance order', RECOVERABLE)
        end

        def common_order_params(symbol)
          {
            symbol: symbol
          }
        end

        def transaction_price(symbol, price, force_smart_intervals, smart_intervals_value, price_in_quote)
          min_price = if price_in_quote
                        @market.minimum_order_price(symbol)
                      else
                        Result::Success.new([@market.minimum_order_volume(symbol).data,
                                             @market.minimum_order_price(symbol).data / @market.current_ask_price(symbol).data].max)
                      end
          return min_price unless min_price.success?

          smart_intervals_value = min_price.data if smart_intervals_value.nil?

          return Result::Success.new([smart_intervals_value, min_price.data].max) if force_smart_intervals

          if price_in_quote
            Result::Success.new([price, min_price.data].max)
          else
            base_step_size = @market.base_step_size(symbol)
            base_decimals = @market.base_decimals(symbol)
            Result::Success.new(
              (([price, min_price.data].max / base_step_size.data).ceil * base_step_size.data).ceil(base_decimals.data)
            )
          end
        end

        def transaction_volume(symbol, price, rate, price_in_quote)
          min_volume = @market.minimum_order_volume(symbol)
          return min_volume unless min_volume.success?

          volume = price_in_quote ? chosen_volume(symbol, price, rate) : Result::Success.new(price)
          return volume unless volume.success?

          Result::Success.new([volume.data, min_volume.data].max)
        end

        def chosen_volume(symbol, price, rate)
          base_step_size = @market.base_step_size(symbol)
          return base_step_size unless base_step_size.success?

          base_decimals = @market.base_decimals(symbol)
          return base_decimals unless base_decimals.success?

          volume = price / rate
          Result::Success.new(
            ((volume / base_step_size.data).ceil * base_step_size.data).ceil(base_decimals.data)
          )
        end

        def parse_response(response)
          return error_to_failure([response['msg']]) if response['msg'].present?

          rate = BigDecimal(response['cummulativeQuoteQty']) / BigDecimal(response['executedQty'])
          Result::Success.new(
            offer_id: response['orderId'],
            rate: rate,
            amount: response['executedQty']
          )
        end

        def parse_base(symbol, amount)
          base_decimals = @market.base_decimals(symbol)
          return base_decimals unless base_decimals.success?

          Result::Success.new("%.#{base_decimals.data}f" % amount)
        end

        def parse_quote(symbol, amount)
          quote_decimals = @market.quote_decimals(symbol)
          return quote_decimals unless quote_decimals.success?

          Result::Success.new("%.#{quote_decimals.data}f" % amount)
        end
      end
    end
  end
end
