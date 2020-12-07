module ExchangeApi
  module Traders
    module Binance
      class MarketTrader < ExchangeApi::Traders::Binance::BaseTrader
        def buy(base:, quote:, price:, force_smart_intervals:)
          symbol = @market.symbol(base, quote)
          buy_params = get_buy_params(symbol, price, force_smart_intervals)
          return buy_params unless buy_params.success?

          place_order(buy_params.data)
        end

        def sell(base:, quote:, price:, force_smart_intervals:)
          symbol = @market.symbol(base, quote)
          sell_params = get_sell_params(symbol, price, force_smart_intervals)
          return sell_params unless sell_params.success?

          place_order(sell_params.data)
        end

        private

        def get_buy_params(symbol, price, force_smart_intervals)
          common_params = common_order_params(symbol, price, force_smart_intervals)
          return common_params unless common_params.success?

          Result::Success.new(common_params.data.merge(side: 'BUY'))
        end

        def get_sell_params(symbol, price, force_smart_intervals)
          common_params = common_order_params(symbol, price, force_smart_intervals)
          return common_params unless common_params.success?

          Result::Success.new(common_params.data.merge(side: 'SELL'))
        end

        def common_order_params(symbol, price, force_smart_intervals)
          price = transaction_price(symbol, price, force_smart_intervals)
          return price unless price.success?

          precision = @market.quote_tick_size_decimals(symbol)
          return precision unless precision.success?

          Result::Success.new(super(symbol)
                                .merge(type: 'MARKET',
                                       quoteOrderQty: price.data.ceil(precision.data)))
        end
      end
    end
  end
end
