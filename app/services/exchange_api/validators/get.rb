module ExchangeApi
  module Validators
    class Get < BaseService
      include ExchangeApi::BinanceEnum

      def call(exchange_id, _key_type = nil)
        exchange = Exchange.find(exchange_id)
        get_trading_key_validator(exchange)
      end

      private

      def get_trading_key_validator(exchange)
        return Fake::Validator.new if Rails.configuration.dry_run

        case exchange.name.downcase
        when 'binance' then Binance::Validator.new(url_base: EU_URL_BASE)
        when 'binance.us' then Binance::Validator.new(url_base: US_URL_BASE)
        when 'kraken' then Kraken::Validator.new
        when 'coinbase' then Coinbase::Validator.new
        when 'bitget' then Bitget::Validator.new
        when 'kucoin' then Kucoin::Validator.new
        when 'bybit' then Bybit::Validator.new
        when 'mexc' then Mexc::Validator.new
        when 'gemini' then Gemini::Validator.new
        when 'bitvavo' then Bitvavo::Validator.new
        when 'hyperliquid' then Hyperliquid::Validator.new
        when 'bingx' then Bingx::Validator.new
        when 'bitrue' then Bitrue::Validator.new
        when 'bitmart' then Bitmart::Validator.new
        end
      end
    end
  end
end
