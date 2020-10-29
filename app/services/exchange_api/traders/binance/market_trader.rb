module ExchangeApi
  module Traders
    module Binance
      class MarketTrader < ExchangeApi::Traders::Binance::BaseTrader
        def buy(base:, quote:, price:, force_smart_intervals:)
          symbol = @market.symbol(base, quote)
          buy_params = get_buy_params(symbol, price, force_smart_intervals)
          place_order(buy_params)
        end

        def sell(base:, quote:, price:, force_smart_intervals:)
          symbol = @market.symbol(base, quote)
          sell_params = get_sell_params(symbol, price, force_smart_intervals)
          place_order(sell_params)
        end

        private

        def get_buy_params(symbol, price, force_smart_intervals)
          common_order_params(symbol, price, force_smart_intervals).merge(side: 'BUY')
        end

        def get_sell_params(symbol, price, force_smart_intervals)
          common_order_params(symbol, price, force_smart_intervals).merge(side: 'SELL')
        end

        def common_order_params(symbol, price, force_smart_intervals)
          price = transaction_price(symbol, price, force_smart_intervals)
          super(symbol).merge(type: 'MARKET', quoteOrderQty: price)
        end
      end
    end
  end
end
