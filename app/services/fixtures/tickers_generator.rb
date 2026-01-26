module Fixtures
  class TickersGenerator < BaseGenerator
    def initialize(exchange)
      super()
      @exchange = exchange
    end

    def generate
      require_coingecko!
      log_info "Generating ticker fixtures for #{@exchange.name_id}..."

      # Fetch tickers from CoinGecko
      log_info "Fetching tickers from CoinGecko..."
      result = coingecko.get_exchange_tickers_by_id(exchange_id: @exchange.coingecko_id)
      if result.failure?
        log_error "Failed to fetch tickers: #{result.error}"
        raise result.error
      end

      @exchange.send(:set_symbol_to_external_id_hash, result.data)

      # Get ticker info from exchange API
      log_info "Fetching ticker details from exchange API..."
      result = @exchange.get_tickers_info(force: true)
      if result.failure?
        log_error "Failed to fetch ticker info: #{result.error}"
        raise result.error
      end

      tickers_info = result.data

      # Filter to only available tickers
      available_tickers = tickers_info.select { |t| t[:available] }
      log_info "Found #{available_tickers.size} available tickers (#{tickers_info.size} total)"

      # Extract relevant data for each ticker
      tickers = available_tickers.map do |ticker_info|
        {
          ticker: ticker_info[:ticker],
          base: ticker_info[:base],
          quote: ticker_info[:quote],
          base_external_id: @exchange.send(:external_id_from_symbol, ticker_info[:base]),
          quote_external_id: @exchange.send(:external_id_from_symbol, ticker_info[:quote]),
          minimum_base_size: ticker_info[:minimum_base_size].to_s,
          minimum_quote_size: ticker_info[:minimum_quote_size].to_s,
          maximum_base_size: ticker_info[:maximum_base_size].to_s,
          maximum_quote_size: ticker_info[:maximum_quote_size].to_s,
          base_decimals: ticker_info[:base_decimals],
          quote_decimals: ticker_info[:quote_decimals],
          price_decimals: ticker_info[:price_decimals]
        }
      end.compact

      # Write to file
      file_path = write_json_file(
        "tickers/#{@exchange.name_id}.json",
        tickers,
        metadata: {
          exchange_name_id: @exchange.name_id,
          exchange_coingecko_id: @exchange.coingecko_id,
          count: tickers.size
        }
      )

      log_info "Ticker fixtures generated successfully for #{@exchange.name_id}"
      Result::Success.new(tickers)
    end

    private

    def coingecko
      @coingecko ||= Coingecko.new(api_key: AppConfig.coingecko_api_key)
    end
  end
end
