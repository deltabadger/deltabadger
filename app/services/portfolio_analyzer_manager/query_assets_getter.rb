require 'utilities/time'
require 'fuzzystringmatch'

module PortfolioAnalyzerManager
  class QueryAssetsGetter < BaseService
    def call(query, portfolio)
      return Result::Success.new([]) if query.blank?

      symbols_info = PortfolioAnalyzerManager::SymbolsInfoGetter.call
      return symbols_info if symbols_info.failure?

      query_downcase = query.downcase
      portfolio_assets_api_ids = portfolio.assets.pluck(:api_id)

      jarow = FuzzyStringMatch::JaroWinkler.create(:native)

      query_assets = symbols_info.data.each_with_object([]) do |symbol, assets|
        next if portfolio_assets_api_ids.include?(symbol['id'].to_s)

        match_distances = get_match_distances(jarow, symbol, query_downcase)
        next if match_distances.first < 0.8

        assets << {
          asset: Asset.new(
            ticker: symbol['symbol']&.upcase,
            name: symbol['name'],
            portfolio_id: portfolio.id,
            category: symbol['symbol_type'],
            color: symbol['color'],
            api_id: symbol['id'].to_s
          ),
          distances: match_distances
        }
      end

      sorted_assets = query_assets.sort_by { |a| [-a[:distances][0], -a[:distances][1], -a[:distances][2]] }.map { |a| a[:asset] }

      Result::Success.new(sorted_assets)
    end

    private

    def get_match_distances(jarow, symbol, query_downcase)
      ticker = symbol['symbol']&.downcase
      name = symbol['name']&.downcase
      isin = symbol['isin']&.downcase
      [
        jarow.getDistance(ticker.to_s, query_downcase),
        jarow.getDistance(name.to_s, query_downcase),
        jarow.getDistance(isin.to_s, query_downcase)
      ].sort.reverse
    end
  end
end
