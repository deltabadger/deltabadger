module ExchangeApi
  module Clients
    module Binance
      class Base < ExchangeApi::Clients::Base
        AddSignature = Struct.new(:app, :api_secret) do
          def call(env)
            signature = OpenSSL::HMAC.hexdigest('sha256', api_secret, env.url.query)
            env.url.query = "#{env.url.query}&signature=#{signature}"
            app.call env
          end
        end
      end
    end
  end
end
