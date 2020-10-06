require 'result'

module ExchangeApi
  module Traders
    module Bitbay
      class LimitTrader < ExchangeApi::Traders::Bitbay::BaseTrader
        def buy(symbol:, price:, percentage:)
          buy_params = get_buy_params(symbol, price, percentage)
          return buy_params unless buy_params.success?

          place_order(symbol, buy_params.data)
        end

        def sell(symbol:, price:, percentage:)
          sell_params = get_sell_params(symbol, price, percentage)
          return sell_params unless sell_params.success?

          place_order(symbol, sell_params.data)
        end

        private

        def place_order(symbol, params)
          response = super
          return response unless response.success?

          Result::Success.new(
            response.data.merge(rate: params[:rate], amount: params[:amount])
          )
        rescue StandardError
          Result::Failure.new('Could not make Bitbay order', RECOVERABLE)
        end

        def parse_response(response)
          if response.fetch('status') == 'Ok'
            Result::Success.new(
              offer_id: response.fetch('offerId')
            )
          else
            error_to_failure(response.fetch('errors'))
          end
        end

        def get_buy_params(symbol, price, percentage)
          rate = @market.current_ask_price(symbol)
          return rate unless rate.success?

          limit_rate = (rate.data * (1 - percentage / 100)).ceil(2)
          amount = transaction_amount(transaction_price(symbol, price), limit_rate)
          Result::Success.new(common_order_params.merge(
                                offerType: 'buy',
                                amount: amount,
                                rate: limit_rate
                              ))
        end

        def get_sell_params(symbol, price, percentage)
          rate = @market.current_bid_price(symbol)
          return rate unless rate.success?

          limit_rate = (rate.data * (1 + percentage / 100)).ceil(2)
          amount = transaction_amount(transaction_price(symbol, price), limit_rate)
          Result::Success.new(common_order_params.merge(
                                offerType: 'sell',
                                amount: amount,
                                rate: limit_rate
                              ))
        end

        def common_order_params
          super.merge(mode: 'limit')
        end
      end
    end
  end
end
