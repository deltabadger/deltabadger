require 'utilities/time'

module PortfolioAnalyzerManager
  class SymbolsInfoGetter < BaseService
    def call(source = 'all')
      start_time = Time.now
      expires_in = Utilities::Time.seconds_to_midnight_utc.seconds + 5.minutes
      end_time = Time.now
      Rails.logger.info("SymbolsInfoGetter.call expires_in took #{end_time - start_time} seconds")

      # start_time = Time.now
      # symbols_data = Rails.cache.read('symbols')
      # end_time = Time.now
      # Rails.logger.info("SymbolsInfoGetter.call read took #{end_time - start_time} seconds")

      start_time = Time.now
      compressed_symbols_data = Rails.cache.fetch('symbols', expires_in: expires_in) do
        Rails.logger.info('Fetching symbols from API')
        client = FinancialDataApiClient.new
        symbols_result = client.symbols(source)
        return symbols_result if symbols_result.failure?

        Zlib::Deflate.deflate(symbols_result.data.to_json)
        # Zlib::Deflate.deflate(Oj.dump(symbols_result.data))
      end
      end_time = Time.now
      Rails.logger.info("SymbolsInfoGetter.call fetch took #{end_time - start_time} seconds")

      start_time = Time.now
      symbols_data = JSON.parse(Zlib::Inflate.inflate(compressed_symbols_data))
      end_time = Time.now
      Rails.logger.info("SymbolsInfoGetter.call parse took #{end_time - start_time} seconds")

      # start_time = Time.now
      # symbols_data = Oj.load(Zlib::Inflate.inflate(compressed_symbols_data))
      # end_time = Time.now
      # Rails.logger.info("SymbolsInfoGetter.call oj took #{end_time - start_time} seconds")

      Result::Success.new(symbols_data)
    end
  end
end
