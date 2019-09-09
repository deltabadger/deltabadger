module ExchangeApi
  class Get < BaseService
    def initialize(exchanges_repository: ExchangesRepository.new)
      @exchanges_repository = exchanges_repository
    end

    def call(api_key)
      exchange = @exchanges_repository.find(api_key.exchange_id)

      case exchange.name
      when 'Kraken'
        ExchangeApi::Clients::Kraken.new(
          api_key: api_key.key,
          api_secret: api_key.secret
        )

      when 'Deribit'  then ExchangeApi::Clients::Deribit.new
      when 'BitBay'   then ExchangeApi::Clients::Bitbay.new
      end
    end
  end
end
