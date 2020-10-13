module ExchangeApi
  module Traders
    module Bitbay
      class MarketTrader < ExchangeApi::Traders::Bitbay::BaseTrader
        def buy(base:, quote:, price:)
          symbol = @market.symbol(base, quote)
          buy_params = get_buy_params(symbol, price)
          place_order(symbol, buy_params)
        end

        def sell(base:, quote:, price:)
          symbol = @market.symbol(base, quote)
          sell_params = get_sell_params(symbol, price)
          place_order(symbol, sell_params)
        end

        private

        def get_buy_params(symbol, price)
          common_order_params.merge(offerType: 'buy', price: transaction_price(symbol, price))
        end

        def get_sell_params(symbol, price)
          common_order_params.merge(offerType: 'sell', price: transaction_price(symbol, price))
        end

        def common_order_params
          super.merge(rate: nil, amount: nil, mode: 'market')
        end
      end
    end
  end
end
