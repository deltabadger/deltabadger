module ExchangeApi
  module WithdrawalInfo
    class Get < BaseService
      include ExchangeApi::BinanceEnum
      include ExchangeApi::FtxEnum
      DISABLE_EXCHANGES_API = ENV.fetch('DISABLE_EXCHANGES_API') == 'true'

      def initialize(exchanges_repository: ExchangesRepository.new)
        @exchanges_repository = exchanges_repository
      end

      def call(api_key)
        exchange = @exchanges_repository.find(api_key.exchange_id)
        return ExchangeApi::WithdrawalInfo::Fake::AccountInfoProcessor.new if DISABLE_EXCHANGES_API

        case exchange.name.downcase
        when 'kraken'
          ExchangeApi::WithdrawalInfo::Kraken::AccountInfoProcessor.new(api_key: api_key.key,
                                                                        api_secret: api_key.secret)
        when 'ftx'
          ExchangeApi::WithdrawalInfo::Ftx::AccountInfoProcessor.new(api_key: api_key.key,
                                                                     api_secret: api_key.secret,
                                                                     url_base: FTX_EU_URL_BASE)
        when 'ftx.us'
          ExchangeApi::WithdrawalInfo::Ftx::AccountInfoProcessor.new(api_key: api_key.key,
                                                                     api_secret: api_key.secret,
                                                                     url_base: FTX_US_URL_BASE)
        end
      end
    end
  end
end
