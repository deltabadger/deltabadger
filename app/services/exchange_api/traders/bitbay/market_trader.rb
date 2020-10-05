module ExchangeApi
  module Traders
    module Bitbay
      class MarketTrader < BaseTrader
        def buy(currency:, price:)
          buy_params = get_buy_params(price)
          place_order(currency, buy_params)
        end

        def sell(currency:, price:)
          sell_params = get_sell_params(price)
          place_order(currency, sell_params)
        end

        private

        def get_buy_params(price)
          common_order_params.merge(offerType: 'buy', price: transaction_price(price))
        end

        def get_sell_params(price)
          common_order_params.merge(offerType: 'sell', price: transaction_price(price))
        end

        def common_order_params
          super.merge(rate: nil, amount: nil, mode: 'market')
        end
      end
    end
  end
end
