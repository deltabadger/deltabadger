require 'singleton'
require 'faye/websocket'
require 'eventmachine'
require 'json'

module ExchangeApi
  module WebSockets
    module Coinbase
      class WebSocket
        include Singleton

        WS_URL = 'wss://advanced-trade-ws.coinbase.com'.freeze

        def initialize
          @bid_prices = Hash.new
          @ask_prices = Hash.new
          @websocket = nil
        end

        def get_bid_price_by_symbol(symbol)
          @bid_prices[symbol]
        end

        def get_ask_price_by_symbol(symbol)
          @ask_prices[symbol]
        end

        def websocket
          @websocket
        end

        def close_ws
          if @websocket && @websocket.ready_state == Faye::WebSocket::API::OPEN
            @websocket.close
          end
        end

        def connect_to_ws(api_key, api_secret, symbol)
          return if @websocket && @websocket.ready_state == Faye::WebSocket::API::OPEN

          timestamp = Time.now.utc.to_i.to_s

          EM.run do
            @websocket = Faye::WebSocket::Client.new(WS_URL)

            @websocket.on :open do |_|
              p [:open]
              open_message = {
                type: "subscribe",
                product_ids: [symbol],
                channel: "level2",
                api_key: api_key,
                timestamp: timestamp,
                signature: ws_signature(api_secret, timestamp, [symbol])
              }

              @websocket.send(open_message.to_json)
            end

            @websocket.on :message do |event|
              on_ws_message(JSON.parse(event&.data))
            end

            @websocket.on :close do |_|
              @websocket = nil
              EM.stop_event_loop if EM.reactor_running?

              # Attempt reconnect after a delay
              sleep 5
              connect_to_ws(api_key, api_secret, symbol) if symbol
            end

          end
        end

        private

        def on_ws_message(event_data)
          symbol = event_data.dig('events', 0, 'product_id')
          side = event_data.dig('events', 0, 'updates', 0, 'side')
          price_level = event_data.dig('events', 0, 'updates', 0, 'price_level')

          if symbol && side && price_level
            price_level = price_level.to_f

            if side == 'bid'
              @bid_prices[symbol] = price_level
            elsif side == 'offer'
              @ask_prices[symbol] = price_level
            end
          end
        end

        def ws_signature(api_secret, timestamp, product_ids)
          payload = "#{timestamp}level2#{product_ids.join(',')}"

          # create a sha256 hmac with the secret
          OpenSSL::HMAC.hexdigest('sha256', api_secret, payload)
        end
      end
    end
  end
end