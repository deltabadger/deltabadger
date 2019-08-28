module ExchangeApi
  class Get < BaseService
    def call(api_key)
      ExchangeApi::Clients::Kraken.new(
        api_key: api_key.key,
        api_secret: api_key.secret
      )
    end
  end
end
