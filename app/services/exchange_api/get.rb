module ExchangeApi
  class Get < BaseService
    DISABLE_EXCHANGES_API = ENV.fetch('DISABLE_EXCHANGES_API') == 'true'

    def initialize(exchanges_repository: ExchangesRepository.new)
      @exchanges_repository = exchanges_repository
    end

    def call(api_key)
      exchange = @exchanges_repository.find(api_key.exchange_id)

      if DISABLE_EXCHANGES_API
        return ExchangeApi::Clients::Fake.new(exchange.name)
      end

      case exchange.name
      when 'Kraken'
        ExchangeApi::Clients::Kraken.new(
          api_key: api_key.key,
          api_secret: api_key.secret
        )
      when 'BitBay'
        ExchangeApi::Clients::Bitbay.new(
          api_key: api_key.key,
          api_secret: api_key.secret
        )
      when 'Deribit' then ExchangeApi::Clients::Deribit.new
      end
    end
  end
end
