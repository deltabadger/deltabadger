require 'result'

module ExchangeApi
  module Traders
    module Probit
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Probit

        def initialize(
          api_key:,
          api_secret:,
          market: ExchangeApi::Markets::Probit::Market.new,
          map_errors: ExchangeApi::MapErrors::Probit.new
        )
          @api_key = api_key
          @api_secret = api_secret
          @market = market
          @map_errors = map_errors
        end

        def fetch_order_by_id(order_id, result_params)
          path = '/api/exchange/v1/order'
          url = API_URL + path
          headers = headers(@api_key, @api_secret)
          market_id = @market.symbol(result_params[:base], result_params[:quote])
          params = {
            "order_id": order_id.to_s,
            "market_id": market_id
          }
          request = Faraday.get(url, params, headers)
          return Result::Failure.new('Waiting for Probit response', NOT_FETCHED) unless request.status == 200

          Result::Success.new(JSON.parse(request.body))
        end

        private

        def place_order(order_params)
          path = '/api/exchange/v1/new_order'.freeze
          url = API_URL + path
          body = order_params.to_json
          headers = headers(@api_key, @api_secret)
          request = Faraday.post(url, body, headers)
          parse_request(request)
        end

        def parse_request(request)
          response = JSON.parse(request.body)
          unless request.status == 200 && request.reason_phrase == 'OK'
            if response['details']['scope'] == 'not allowed scope'
              return error_to_failure([response['details']['scope']])
            end

            return error_to_failure([response['errorCode']])
          end
          response_data = response['data']
          order_id = response_data['id']
          Result::Success.new(offer_id: order_id)
        end

        def transaction_cost(price, symbol, force_smart_intervals, smart_intervals_value)
          min_price = @market.minimum_order_price(symbol)
          return min_price unless min_price.success?

          quote_decimals = @market.quote_decimals(symbol)
          return quote_decimals unless quote_decimals.success?

          smart_intervals_value = min_price.data if smart_intervals_value.nil?
          smart_intervals_value = smart_intervals_value.ceil(quote_decimals.data)


          return Result::Success.new([smart_intervals_value, min_price.data].max) if force_smart_intervals

          Result::Success.new([min_price.data, price].max.ceil(quote_decimals.data).to_d)
        end

        def transaction_quantity(price, symbol, force_smart_intervals, smart_intervals_value, price_in_base = false)
          min_price = @market.minimum_order_quantity(symbol)
          return min_price unless min_price.success?

          min_order_price = @market.minimum_order_price(symbol)
          current_price = @market.current_price(symbol)
          return min_order_price unless min_order_price.success? && current_price.success?

          price /= current_price.data if price_in_base
          min_price_by_quote = min_order_price.data / current_price.data
          min_price = Result::Success.new(min_price_by_quote) if min_price.data < min_price_by_quote
          base_decimals = @market.base_decimals(symbol)
          return base_decimals unless base_decimals.success?

          smart_intervals_value = if smart_intervals_value.nil?
                                    min_price.data
                                  else
                                    (smart_intervals_value / current_price.data).ceil(base_decimals.data)
                                  end
          smart_intervals_value = smart_intervals_value.ceil(base_decimals.data)

          return Result::Success.new([smart_intervals_value, min_price.data].max) if force_smart_intervals

          Result::Success.new([min_price.data, price].max.ceil(base_decimals.data).to_d)
        end
      end
    end
  end
end
