module ExchangeApi
  module Traders
    module Ftx
      class MarketTrader < ExchangeApi::Traders::Ftx::BaseTrader
        def buy(base:, quote:, price:, force_smart_intervals:, smart_intervals_value:, use_subaccount: false, selected_subaccount: '')
          symbol = @market.symbol(base, quote)
          buy_params = get_buy_params(symbol, price, force_smart_intervals, smart_intervals_value)
          return buy_params unless buy_params.success?

          place_order(buy_params.data, use_subaccount, selected_subaccount)
        end

        def sell(base:, quote:, price:, force_smart_intervals:, smart_intervals_value:, is_legacy:, use_subaccount: false, selected_subaccount: '')
          symbol = @market.symbol(base, quote)
          sell_params = get_sell_params(symbol, price, force_smart_intervals, smart_intervals_value, is_legacy)
          return sell_params unless sell_params.success?

          place_order(sell_params.data, use_subaccount, selected_subaccount)
        end

        private

        def get_buy_params(symbol, price, force_smart_intervals, smart_intervals_value)
          rate = @market.current_ask_price(symbol)
          return rate unless rate.success?

          volume = smart_volume(symbol, price, rate.data, force_smart_intervals, smart_intervals_value, true)
          return volume unless volume.success?

          Result::Success.new(
            common_order_params(symbol).merge(
              side: 'buy',
              size: volume.data
            )
          )
        end

        def get_sell_params(symbol, price, force_smart_intervals, smart_intervals_value, price_in_quote)
          rate = @market.current_ask_price(symbol)
          return rate unless rate.success?

          volume = smart_volume(symbol, price, rate.data, force_smart_intervals, smart_intervals_value, price_in_quote)
          return volume unless volume.success?

          Result::Success.new(
            common_order_params(symbol).merge(
              side: 'sell',
              size: volume.data
            )
          )
        end

        def common_order_params(symbol)
          super.merge(type: 'market', price: nil)
        end
      end
    end
  end
end
