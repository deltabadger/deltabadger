require 'result'

module ExchangeApi
  module Clients
    module Kraken
      class MarketTrader < BaseTrader
        def buy(currency:, price:)
          buy_params = get_buy_params(currency, price)
          return buy_params unless buy_params.success?

          place_order(buy_params)
        end

        def sell(currency:, price:)
          sell_params = get_sell_params(currency, price)
          return sell_params unless sell_params.success?

          place_order(sell_params)
        end

        private

        def get_buy_params(currency, price)
          volume = smart_volume(price, current_ask_price(currency))
          return volume unless volume.success?

          Result::Success.new(common_order_params(currency).merge(type: 'buy', volume: volume))
        end

        def get_sell_params(currency, price)
          volume = smart_volume(price, current_bid_price(currency))
          return volume unless volume.success?

          Result::Success.new(common_order_params(currency).merge(type: 'sell', volume: volume))
        end

        def common_order_params(currency)
          super(currency).merge(ordertype: 'market')
        end
      end
    end
  end
end
