module PortfolioAnalyzerManager
  class QueryAssetsGetter < BaseService
    def call(query, portfolio)
      return Result::Success.new([]) if query.blank?

      all_asset_tickers = PortfolioAnalyzerManager::AllAssetTickersGetter.call
      return all_asset_tickers if all_asset_tickers.failure?

      available_asset_tickers = all_asset_tickers.data.map(&:upcase) - portfolio.assets.map(&:ticker)
      query_assets_tickers = available_asset_tickers.filter { |a| a.include?(query.upcase) }
      query_assets = query_assets_tickers.map { |a| Asset.new(ticker: a, portfolio_id: portfolio.id) }
      Result::Success.new(query_assets)
    end
  end
end
