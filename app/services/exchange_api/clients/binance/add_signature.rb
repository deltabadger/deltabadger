module ExchangeApi
  module Clients
    class Binance
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
