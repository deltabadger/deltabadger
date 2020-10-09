module ExchangeApi
  module Traders
    module Binance
      class MarketTrader < ExchangeApi::Traders::Binance::BaseTrader
        def buy(currency:, price:)
          buy_params = get_buy_params(currency, price)
          place_order(buy_params)
        end

        def sell(currency:, price:)
          sell_params = get_sell_params(currency, price)
          place_order(sell_params)
        end

        private

        def get_buy_params(currency, price)
          common_order_params(currency, price).merge(side: 'BUY')
        end

        def get_sell_params(currency, price)
          common_order_params(currency, price).merge(side: 'SELL')
        end

        def common_order_params(currency, price)
          price = transaction_price(currency, price)
          super(currency).merge(type: 'MARKET', quoteOrderQty: price)
        end
      end
    end
  end
end
