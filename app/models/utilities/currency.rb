module Utilities
  module Currency
    STABLECOIN_IDS = {
      'USDC' => 'usd-coin',
      'USDT' => 'tether',
      'DAI' => 'dai',
      'BUSD' => 'binance-usd',
      'TUSD' => 'true-usd',
      'USDP' => 'paxos-standard',
      'GUSD' => 'gemini-dollar',
      'FRAX' => 'frax',
      'LUSD' => 'liquity-usd',
      'USDD' => 'usdd',
      'PYUSD' => 'paypal-usd'
    }.freeze

    CACHE_DURATION = 60.seconds

    class << self
      # Convert amount from one currency to another
      # @param amount [Numeric] The amount to convert
      # @param from [String] Source currency symbol (e.g., 'EUR', 'BTC', 'USDC')
      # @param to [String] Target currency symbol, default 'USD'
      # @return [Result::Success, Result::Failure] Result with converted amount or error
      def convert(amount, from:, to: 'USD')
        return Result::Success.new(amount) if from.upcase == to.upcase
        return Result::Success.new(0.0) if amount.zero?

        result = exchange_rate(from: from, to: to)
        return result if result.failure?

        Result::Success.new(amount * result.data)
      end

      # Get exchange rate between two currencies
      # @param from [String] Source currency symbol
      # @param to [String] Target currency symbol, default 'USD'
      # @return [Result::Success, Result::Failure] Result with exchange rate or error
      def exchange_rate(from:, to: 'USD')
        from = from.upcase
        to = to.upcase
        return Result::Success.new(1.0) if from == to

        cache_key = "exchange_rate_#{from}_to_#{to}"
        Rails.cache.fetch(cache_key, expires_in: CACHE_DURATION) do
          calculate_exchange_rate(from, to)
        end
      end

      # Batch convert multiple amounts efficiently (single API call for rates)
      # @param amounts_by_currency [Hash] Hash of { 'EUR' => 100, 'BTC' => 0.5 }
      # @param to [String] Target currency symbol, default 'USD'
      # @return [Result::Success, Result::Failure] Result with total converted amount or error
      def batch_convert(amounts_by_currency, to: 'USD')
        to = to.upcase
        total = 0.0

        amounts_by_currency.each do |currency, amount|
          next if amount.zero?

          result = convert(amount, from: currency, to: to)
          return result if result.failure?

          total += result.data
        end

        Result::Success.new(total)
      end

      private

      def calculate_exchange_rate(from, to)
        from_asset = find_asset(from)
        to_asset = find_asset(to)

        # Both are fiat currencies - use exchange_rates endpoint
        return fiat_to_fiat_rate(from, to) if fiat?(from, from_asset) && fiat?(to, to_asset)

        # From crypto/stablecoin to fiat (most common case)
        return crypto_to_fiat_rate(from, from_asset, to) if crypto_or_stablecoin?(from, from_asset) && fiat?(to, to_asset)

        # From fiat to crypto/stablecoin
        if fiat?(from, from_asset) && crypto_or_stablecoin?(to, to_asset)
          result = crypto_to_fiat_rate(to, to_asset, from)
          return result if result.failure?

          return Result::Success.new(1.0 / result.data)
        end

        # Both are crypto - convert through USD
        return crypto_to_crypto_rate(from, from_asset, to, to_asset) if crypto_or_stablecoin?(from, from_asset) && crypto_or_stablecoin?(to, to_asset)

        Result::Failure.new("Unable to determine conversion path from #{from} to #{to}")
      end

      def fiat_to_fiat_rate(from, to)
        result = MarketData.get_exchange_rates
        return result if result.failure?

        rates = result.data
        from_key = from.downcase
        to_key = to.downcase

        from_rate = rates.dig(from_key, 'value')
        to_rate = rates.dig(to_key, 'value')

        return Result::Failure.new("Exchange rate not found for #{from} or #{to}") if from_rate.nil? || to_rate.nil?

        # Rates are BTC-based, so: from_currency -> BTC -> to_currency
        # If 1 BTC = X EUR and 1 BTC = Y USD, then 1 EUR = Y/X USD
        Result::Success.new(to_rate / from_rate)
      end

      def crypto_to_fiat_rate(crypto_symbol, crypto_asset, fiat_symbol)
        coin_id = coingecko_id_for(crypto_symbol, crypto_asset)
        return Result::Failure.new("No CoinGecko ID found for #{crypto_symbol}") if coin_id.nil?

        result = MarketData.get_price(coin_id: coin_id, currency: fiat_symbol.downcase)
        return result if result.failure?

        Result::Success.new(result.data)
      end

      def crypto_to_crypto_rate(from_symbol, from_asset, to_symbol, to_asset)
        # Convert both to USD first, then calculate cross rate
        from_result = crypto_to_fiat_rate(from_symbol, from_asset, 'USD')
        return from_result if from_result.failure?

        to_result = crypto_to_fiat_rate(to_symbol, to_asset, 'USD')
        return to_result if to_result.failure?

        Result::Success.new(from_result.data / to_result.data)
      end

      def find_asset(symbol)
        Asset.find_by(symbol: symbol.upcase)
      end

      def fiat?(symbol, asset)
        asset&.category == 'Currency' || FIAT_SYMBOLS.include?(symbol.upcase)
      end

      def crypto_or_stablecoin?(symbol, asset)
        asset&.category == 'Cryptocurrency' || STABLECOIN_IDS.key?(symbol.upcase)
      end

      def coingecko_id_for(symbol, asset)
        # Check stablecoins first
        return STABLECOIN_IDS[symbol.upcase] if STABLECOIN_IDS.key?(symbol.upcase)

        # Use asset's external_id if available
        asset&.external_id
      end

      FIAT_SYMBOLS = %w[
        USD EUR GBP JPY CAD AUD CHF CNY INR MXN BRL
        KRW SGD HKD NOK SEK DKK NZD ZAR RUB TRY PLN
        THB IDR MYR PHP CZK ILS ARS CLP COP PEN UAH
      ].freeze
    end
  end
end
