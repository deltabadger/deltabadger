module ExchangeApi
  module Validators
    class Get < BaseService
      include ExchangeApi::BinanceEnum
      include ExchangeApi::FtxEnum
      DISABLE_EXCHANGES_API = ENV.fetch('DISABLE_EXCHANGES_API') == 'true'

      def initialize(exchanges_repository: ExchangesRepository.new)
        @exchanges_repository = exchanges_repository
      end

      def call(exchange_id, key_type)
        exchange = @exchanges_repository.find(exchange_id)
        key_type == 'trading' ? get_trading_key_validator(exchange) : get_withdrawal_key_validator(exchange)
      end

      private

      def get_trading_key_validator(exchange)
        return Fake::Validator.new if DISABLE_EXCHANGES_API

        case exchange.name.downcase
        when 'binance'
          Binance::Validator.new(url_base: EU_URL_BASE)
        when 'binance.us'
          Binance::Validator.new(url_base: US_URL_BASE)
        when 'bitbay'
          Bitbay::Validator.new
        when 'kraken'
          Kraken::Validator.new
        when 'coinbase pro'
          CoinbasePro::Validator.new
        when 'gemini'
          Gemini::Validator.new
        when 'ftx'
          Ftx::Validator.new(url_base: FTX_EU_URL_BASE)
        when 'ftx.us'
          Ftx::Validator.new(url_base: FTX_US_URL_BASE)
        when 'bitso'
          Bitso::Validator.new
        when 'kucoin'
          Kucoin::Validator.new
        when 'bitfinex'
          Bitfinex::Validator.new
        when 'bitstamp'
          Bitstamp::Validator.new
        end
      end

      def get_withdrawal_key_validator(exchange)
        return Fake::WithdrawalValidator.new if DISABLE_EXCHANGES_API

        case exchange.name.downcase
        when 'binance'
          Binance::WithdrawalValidator.new(url_base: EU_WITHDRAWAL_URL_BASE)
        when 'binance.us'
          Binance::WithdrawalValidator.new(url_base: US_WITHDRAWAL_URL_BASE)
        when 'bitbay'
          Bitbay::WithdrawalValidator.new
        when 'kraken'
          Kraken::WithdrawalValidator.new
        when 'coinbase pro'
          CoinbasePro::WithdrawalValidator.new
        when 'gemini'
          Gemini::WithdrawalValidator.new
        when 'ftx'
          Ftx::WithdrawalValidator.new(url_base: FTX_EU_URL_BASE)
        when 'ftx.us'
          Ftx::WithdrawalValidator.new(url_base: FTX_US_URL_BASE)
        when 'bitso'
          Bitso::WithdrawalValidator.new
        when 'kucoin'
          Kucoin::WithdrawalValidator.new
        when 'bitfinex'
          Bitfinex::WithdrawalValidator.new
        when 'bitstamp'
          Bitstamp::WithdrawalValidator.new
        end
      end
    end
  end
end
