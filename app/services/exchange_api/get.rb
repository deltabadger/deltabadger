module ExchangeApi
  class Get < BaseService
    DISABLE_EXCHANGES_API = ENV.fetch('DISABLE_EXCHANGES_API') == 'true'

    def initialize(exchanges_repository: ExchangesRepository.new)
      @exchanges_repository = exchanges_repository
    end

    def call(api_key)
      exchange = @exchanges_repository.find(api_key.exchange_id)

      return ExchangeApi::Clients::Fake.new(exchange.name) if DISABLE_EXCHANGES_API

      case exchange.name.downcase
      when 'binance'
        ExchangeApi::Clients::Binance.new(
          api_key: api_key.key,
          api_secret: api_key.secret
        )
      when 'bitbay'
        ExchangeApi::Clients::Bitbay.new(
          api_key: api_key.key,
          api_secret: api_key.secret
        )
      when 'kraken'
        ExchangeApi::Clients::Kraken.new(
          api_key: api_key.key,
          api_secret: api_key.secret,
          options: { german_trading_agreement: api_key.german_trading_agreement }
        )
      end
    end
  end
end
