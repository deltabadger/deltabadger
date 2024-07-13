require 'utilities/time'

module PortfolioAnalyzerManager
  class SymbolsInfoGetter < BaseService
    def call(source = 'all')
      expires_in = Utilities::Time.seconds_to_midnight_utc.seconds + 5.minutes

      # start_time = Time.now
      compressed_symbols_data = Rails.cache.fetch('symbols', expires_in: expires_in) do
        Rails.logger.info('Fetching symbols from API')
        client = FinancialDataApiClient.new
        symbols_result = client.symbols(source)
        return symbols_result if symbols_result.failure?

        Zlib::Deflate.deflate(symbols_result.data.to_json)
        # Zlib::Deflate.deflate(Oj.dump(symbols_result.data))
      end
      symbols_data = JSON.parse(Zlib::Inflate.inflate(compressed_symbols_data))
      # end_time = Time.now
      # Rails.logger.info("SymbolsInfoGetter.call compressed fetch & parse took #{end_time - start_time} seconds")

      Result::Success.new(symbols_data)
    end
  end
end
