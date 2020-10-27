module ExchangeApi
  module Traders
    module Bitbay
      class MarketTrader < ExchangeApi::Traders::Bitbay::BaseTrader
        def buy(base:, quote:, price:, force:)
          symbol = @market.symbol(base, quote)
          buy_params = get_buy_params(symbol, price, force)
          return buy_params unless buy_params.success?

          place_order(symbol, buy_params.data)
        end

        def sell(base:, quote:, price:, force:)
          symbol = @market.symbol(base, quote)
          sell_params = get_sell_params(symbol, price, force)
          return sell_params unless sell_params.success?

          place_order(symbol, sell_params.data)
        end

        private

        def get_buy_params(symbol, price, force)
          price_above_minimums = transaction_price(symbol, price, force)
          return price_above_minimums unless price_above_minimums.success?

          Result::Success.new(
            common_order_params.merge(
              offerType: 'buy',
              price: price_above_minimums.data
            )
          )
        end

        def get_sell_params(symbol, price, force)
          price_above_minimums = transaction_price(symbol, price, force)
          return price_above_minimums unless price_above_minimums.success?

          Result::Success.new(
            common_order_params.merge(
              offerType: 'sell',
              price: price_above_minimums.data
            )
          )
        end

        def common_order_params
          super.merge(rate: nil, amount: nil, mode: 'market')
        end
      end
    end
  end
end
