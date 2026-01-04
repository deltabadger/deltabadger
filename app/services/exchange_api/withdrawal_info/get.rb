module ExchangeApi
  module WithdrawalInfo
    class Get < BaseService
      include ExchangeApi::BinanceEnum

      def call(api_key)
        exchange = Exchange.find(api_key.exchange_id)
        return ExchangeApi::WithdrawalInfo::Fake::AccountInfoProcessor.new if Rails.configuration.dry_run

        case exchange.name.downcase
        when 'kraken'
          ExchangeApi::WithdrawalInfo::Kraken::AccountInfoProcessor.new(api_key: api_key.key,
                                                                        api_secret: api_key.secret)
        end
      end
    end
  end
end
