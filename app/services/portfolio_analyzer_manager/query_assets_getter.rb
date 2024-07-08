require 'utilities/time'

module PortfolioAnalyzerManager
  class QueryAssetsGetter < BaseService
    CUSTOM_CATEGORIES = {
      '^GSPC': 'index',
      '^OEX': 'index',
      '^DJI': 'index',
      '^IXIC': 'index',
      '^RUT': 'index',
      '^FVX': 'bond',
      '^TNX': 'bond',
      '^TYX': 'bond'
    }.freeze

    def call(query, portfolio)
      return Result::Success.new([]) if query.blank?

      symbols_info = PortfolioAnalyzerManager::SymbolsInfoGetter.call
      return symbols_info if symbols_info.failure?

      query_assets = symbols_info.data.map do |symbol|
        next if asset_already_in_portfolio?(portfolio, symbol) || !matches_query?(symbol, query)

        Asset.new(
          ticker: symbol['symbol']&.upcase,
          name: symbol['name'],
          portfolio_id: portfolio.id,
          category: symbol['symbol_type'],
          color: symbol['color'],
          api_id: api_id(symbol)
        )
      end.compact
      Result::Success.new(query_assets)
    end

    private

    def api_id(symbol)
      symbol['id'].to_s
    end

    def get_remote_symbols_info
      expires_in = Utilities::Time.seconds_to_midnight_utc.seconds + 5.minutes
      Rails.cache.fetch('symbols', expires_in: expires_in) do
        client = FinancialDataApiClient.new
        symbols_result = client.symbols('all')
        return [] if symbols_result.failure?

        symbols_result.data
      end
    end

    def asset_already_in_portfolio?(portfolio, symbol)
      portfolio_assets_api_ids = portfolio.assets.map(&:api_id)
      portfolio_assets_api_ids.include?(api_id(symbol))
    end

    def matches_query?(symbol, query)
      query = query.upcase
      ticker = symbol['symbol']&.upcase
      name = symbol['name']&.upcase
      isin = symbol['isin']&.upcase
      ticker&.include?(query) || name&.include?(query) || isin&.include?(query)
    end
  end
end
