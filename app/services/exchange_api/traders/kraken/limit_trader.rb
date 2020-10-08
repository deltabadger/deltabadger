require 'result'

module ExchangeApi
  module Traders
    module Kraken
      class LimitTrader < BaseTrader
        def buy(currency:, price:, percentage:)
          buy_params = get_buy_params(currency, price, percentage)
          return buy_params unless buy_params.success?

          place_order(buy_params.data)
        end

        def sell(currency:, price:, percentage:)
          sell_params = get_sell_params(currency, price, percentage)
          return sell_params unless sell_params.success?

          place_order(sell_params.data)
        end

        private

        def parse_response(response)
          created_order = response.fetch('result')
          offer_id = created_order.fetch('txid').first
          order_data = orders.fetch(offer_id)
          rate = order_data.fetch('descr').fetch('price').to_f
          amount = order_data.fetch('vol').to_f
          {
            offer_id: offer_id,
            rate: rate,
            amount: amount
          }
        end

        def orders
          @client.open_orders.dig('result', 'open')
        end

        def get_buy_params(currency, price, percentage)
          rate = current_ask_price(currency)
          return rate unless rate.success?

          limit_rate = (rate.data * (1 - percentage / 100)).ceil(1)
          volume = smart_volume(price, limit_rate)
          return volume unless volume.success?

          Result::Success.new(common_order_params(currency).merge(
                                type: 'buy',
                                volume: volume.data,
                                price: limit_rate
                              ))
        end

        def get_sell_params(currency, price, percentage)
          rate = current_bid_price(currency)
          return rate unless rate.success?

          limit_rate = (rate.data * (1 + percentage / 100)).ceil(1)
          volume = smart_volume(price, limit_rate)
          return volume unless volume.success?

          Result::Success.new(common_order_params(currency).merge(
                                type: 'sell',
                                volume: volume.data,
                                price: limit_rate
                              ))
        end

        def common_order_params(currency)
          super(currency).merge(ordertype: 'limit')
        end
      end
    end
  end
end
