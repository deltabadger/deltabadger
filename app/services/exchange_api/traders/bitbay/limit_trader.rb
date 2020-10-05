require 'result'

module ExchangeApi
  module Traders
    module Bitbay
      class LimitTrader < BaseTrader
        def buy(currency:, price:, percentage:)
          buy_params = get_buy_params(currency, price, percentage)
          return buy_params unless buy_params.success?

          place_order(currency, buy_params.data)
        end

        def sell(currency:, price:, percentage:)
          sell_params = get_sell_params(currency, price, percentage)
          return sell_params unless sell_params.success?

          place_order(currency, sell_params.data)
        end

        private

        def place_order(currency, params)
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
              offer_id: response.fetch('offerId'),
            )
          else
            error_to_failure(response.fetch('errors'))
          end
        end

        def get_buy_params(currency, price, percentage)
          rate = current_ask_price(currency)
          return rate unless rate.success?

          limit_rate = (rate.data * (1 - percentage / 100)).ceil(2)
          amount = transaction_amount(transaction_price(price), limit_rate)
          Result::Success.new(common_order_params.merge(
                                offerType: 'buy',
                                amount: amount,
                                rate: limit_rate
                              ))
        end

        def get_sell_params(currency, price, percentage)
          rate = current_bid_price(currency)
          return rate unless rate.success?

          limit_rate = (rate.data * (1 + percentage / 100)).ceil(2)
          amount = transaction_amount(transaction_price(price), limit_rate)
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
