require 'utilities/time'

module PortfolioAnalyzerManager
  class AllAssetTickersGetter < BaseService
    def call
      source = 'binance'
      timeframe = '1d'
      expires_in = Utilities::Time.seconds_to_midnight_utc.seconds
      all_symbols = Rails.cache.fetch("symbols_#{source}_#{timeframe}", expires_in: expires_in) do
        client = FinancialDataApiClient.new
        symbols_result = client.symbols(source, timeframe)
        return symbols_result if symbols_result.failure?

        symbols_result.data
      end
      Result::Success.new(all_symbols.map { |s| s[0...-4] }.sort!)
    end
  end
end
