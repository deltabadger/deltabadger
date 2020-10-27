module ExchangeApi
  module Traders
    module Binance
      class MarketTrader < ExchangeApi::Traders::Binance::BaseTrader
        def buy(base:, quote:, price:, force:)
          symbol = @market.symbol(base, quote)
          buy_params = get_buy_params(symbol, price, force)
          place_order(buy_params)
        end

        def sell(base:, quote:, price:, force:)
          symbol = @market.symbol(base, quote)
          sell_params = get_sell_params(symbol, price, force)
          place_order(sell_params)
        end

        private

        def get_buy_params(symbol, price, force)
          common_order_params(symbol, price, force).merge(side: 'BUY')
        end

        def get_sell_params(symbol, price, force)
          common_order_params(symbol, price, force).merge(side: 'SELL')
        end

        def common_order_params(symbol, price, force)
          price = transaction_price(symbol, price, force)
          super(symbol).merge(type: 'MARKET', quoteOrderQty: price)
        end
      end
    end
  end
end
