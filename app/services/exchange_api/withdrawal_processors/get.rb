module ExchangeApi
  module WithdrawalProcessors
    class Get < BaseService
      include ExchangeApi::BinanceEnum
      include ExchangeApi::FtxEnum
      DISABLE_EXCHANGES_API = ENV.fetch('DISABLE_EXCHANGES_API') == 'true'

      def call(api_key)
        exchange = Exchange.find(api_key.exchange_id)
        return ExchangeApi::WithdrawalProcessors::Fake::RequestProcessor.new if DISABLE_EXCHANGES_API

        case exchange.name.downcase
        when 'kraken'
          ExchangeApi::WithdrawalProcessors::Kraken::RequestProcessor.new(api_key: api_key.key,
                                                                          api_secret: api_key.secret)
        when 'ftx'
          ExchangeApi::WithdrawalProcessors::Ftx::RequestProcessor.new(api_key: api_key.key,
                                                                       api_secret: api_key.secret,
                                                                       url_base: FTX_EU_URL_BASE)
        when 'ftx.us'
          ExchangeApi::WithdrawalProcessors::Ftx::RequestProcessor.new(api_key: api_key.key,
                                                                       api_secret: api_key.secret,
                                                                       url_base: FTX_US_URL_BASE)
        end
      end
    end
  end
end
