module ExchangeApi
  class Get < BaseService
    DISABLE_EXCHANGES_API = ENV.fetch('DISABLE_EXCHANGES_API') == 'true'

    def initialize(exchanges_repository: ExchangesRepository.new)
      @exchanges_repository = exchanges_repository
    end

    def call(api_key) # rubocop:disable Metrics/MethodLength
      exchange = @exchanges_repository.find(api_key.exchange_id)

      return ExchangeApi::Clients::Fake.new(exchange.name) if DISABLE_EXCHANGES_API

      case exchange.name
      when 'Kraken'
        ExchangeApi::Clients::Kraken.new(
          api_key: api_key.key,
          api_secret: api_key.secret,
          options: { german_trading_agreement: api_key.german_trading_agreement }
        )
      when 'Binance'
        ExchangeApi::Clients::Binance.new(
          api_key: api_key.key,
          api_secret: api_key.secret
        )
      when 'BitBay'
        ExchangeApi::Clients::Bitbay.new(
          api_key: api_key.key,
          api_secret: api_key.secret
        )
      end
    end
  end
end
