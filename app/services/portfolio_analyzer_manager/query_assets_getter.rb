require 'utilities/time'

module PortfolioAnalyzerManager
  class QueryAssetsGetter < BaseService
    def call(query, portfolio)
      return Result::Success.new([]) if query.blank?

      symbols_info = PortfolioAnalyzerManager::SymbolsInfoGetter.call
      return symbols_info if symbols_info.failure?

      query_upcase = query.upcase
      portfolio_assets_api_ids = portfolio.assets.pluck(:api_id)

      query_assets = symbols_info.data.each_with_object([]) do |symbol, assets|
        next if portfolio_assets_api_ids.include?(symbol['id'].to_s)
        next unless matches_query?(symbol, query_upcase)

        assets << Asset.new(
          ticker: symbol['symbol']&.upcase,
          name: symbol['name'],
          portfolio_id: portfolio.id,
          category: symbol['symbol_type'],
          color: symbol['color'],
          api_id: symbol['id'].to_s
        )
      end

      Result::Success.new(query_assets)
    end

    private

    def matches_query?(symbol, query)
      attributes = [symbol['symbol'], symbol['name'], symbol['isin']].compact.map(&:upcase)
      attributes.any? { |attr| attr.include?(query) }
    end
  end
end
