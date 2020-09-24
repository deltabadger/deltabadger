module ExchangeApi
  module Clients
    module Bitbay
      class MarketTrader < BaseTrader
        def buy(currency:, price:)
          buy_params = get_buy_params(price)
          place_order(currency, buy_params.to_json)
        end

        def sell(currency:, price:)
          sell_params = get_sell_params(price)
          place_order(currency, sell_params.to_json)
        end

        private

        def get_buy_params(price)
          common_order_params(price).merge(offerType: 'buy')
        end

        def get_sell_params(price)
          common_order_params(price).merge(offerType: 'sell')
        end

        def common_order_params(price)
          super(price).merge(rate: nil, mode: 'market')
        end
      end
    end
  end
end
