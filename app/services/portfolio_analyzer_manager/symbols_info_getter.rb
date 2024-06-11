require 'utilities/time'

module PortfolioAnalyzerManager
  class SymbolsInfoGetter < BaseService
    def call(source = 'all')
      expires_in = Utilities::Time.seconds_to_midnight_utc.seconds + 5.minutes
      symbols_data = Rails.cache.fetch('symbols', expires_in: expires_in) do
        client = FinancialDataApiClient.new
        symbols_result = client.symbols(source)
        return symbols_result if symbols_result.failure?

        symbols_result.data
      end
      Result::Success.new(symbols_data)
    end
  end
end
