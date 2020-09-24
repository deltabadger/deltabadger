module ExchangeApi
  module Clients
    module Binance
      class Base < ExchangeApi::Clients::Base
        AddTimestamp = Struct.new(:app, :api_secret) do
          def call(env)
            timestamp = DateTime.now.strftime('%Q')
            env.url.query = "#{env.url.query}&timestamp=#{timestamp}"
            app.call env
          end
        end
      end
    end
  end
end
