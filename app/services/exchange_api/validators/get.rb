module ExchangeApi
  module Validators
    class Get < BaseService
      include ExchangeApi::BinanceEnum
      include ExchangeApi::FtxEnum
      DISABLE_EXCHANGES_API = ENV.fetch('DISABLE_EXCHANGES_API') == 'true'

      def call(exchange_id, key_type)
        exchange = Exchange.find(exchange_id)
        key_type == 'withdrawal' ? get_withdrawal_key_validator(exchange) : get_trading_key_validator(exchange)
      end

      private

      # rubocop:disable Metrics/CyclomaticComplexity
      def get_trading_key_validator(exchange)
        return Fake::Validator.new if DISABLE_EXCHANGES_API

        case exchange.name.downcase
        when 'binance' then Binance::Validator.new(url_base: EU_URL_BASE)
        when 'binance.us' then Binance::Validator.new(url_base: US_URL_BASE)
        when 'zonda' then Zonda::Validator.new
        when 'kraken' then Kraken::Validator.new
        when 'coinbase pro' then CoinbasePro::Validator.new
        when 'coinbase' then Coinbase::Validator.new
        when 'gemini' then Gemini::Validator.new
        when 'ftx' then Ftx::Validator.new(url_base: FTX_EU_URL_BASE)
        when 'ftx.us' then Ftx::Validator.new(url_base: FTX_US_URL_BASE)
        when 'bitso' then Bitso::Validator.new
        when 'kucoin' then Kucoin::Validator.new
        when 'bitfinex' then Bitfinex::Validator.new
        when 'bitstamp' then Bitstamp::Validator.new
        when 'probit global' then Probit::Validator.new
        when 'probit' then Probit::Validator.new
        end
      end
      # rubocop:enable Metrics/CyclomaticComplexity

      def get_withdrawal_key_validator(exchange)
        return Fake::WithdrawalValidator.new if DISABLE_EXCHANGES_API

        case exchange.name.downcase
        when 'kraken'
          ExchangeApi::Validators::Kraken::WithdrawalValidator.new
        when 'ftx'
          ExchangeApi::Validators::Ftx::WithdrawalValidator.new(url_base: FTX_EU_URL_BASE)
        when 'ftx.us'
          ExchangeApi::Validators::Ftx::WithdrawalValidator.new(url_base: FTX_US_URL_BASE)
        end
      end
    end
  end
end
