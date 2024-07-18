require 'utilities/time'

module PortfolioAnalyzerManager
  class QueryAssetsGetter < BaseService
    def call(query, portfolio)
      return Result::Success.new([]) if query.blank?

      start_time = Time.now
      symbols_info = PortfolioAnalyzerManager::SymbolsInfoGetter.call
      end_time = Time.now
      Rails.logger.info("SymbolsInfoGetter.call took #{end_time - start_time} seconds")
      return symbols_info if symbols_info.failure?

      start_time = Time.now
      query_downcase = query.downcase
      portfolio_assets_api_ids = portfolio.assets.pluck(:api_id)
      end_time = Time.now
      Rails.logger.info("portfolio_assets_api_ids took #{end_time - start_time} seconds")

      start_time = Time.now
      query_assets = symbols_info.data.each_with_object([]) do |symbol, assets|
        next if portfolio_assets_api_ids.include?(symbol['id'].to_s)

        match_similarities = get_match_similarities(symbol, query_downcase)
        next if match_similarities.first < 0.8

        # assets << {
        #   asset: Asset.new(
        #     ticker: symbol['symbol']&.upcase,
        #     name: symbol['name'],
        #     portfolio_id: portfolio.id,
        #     category: symbol['category'],
        #     color: symbol['color'],
        #     api_id: symbol['id'].to_s
        #   ),
        #   similarities: match_similarities
        # }
        assets << {
          asset: {
            ticker: symbol['symbol']&.upcase,
            name: symbol['name'],
            portfolio_id: portfolio.id,
            category: symbol['category'],
            color: symbol['color'],
            api_id: symbol['id'].to_s,
            country: symbol['country'],
            exchange: symbol['exchange'],
            url: ("https://www.coingecko.com/coins/#{symbol['external_id']}" if symbol['external_id'].present?)
          },
          similarities: match_similarities
        }
      end
      end_time = Time.now
      Rails.logger.info("query_assets took #{end_time - start_time} seconds")

      start_time = Time.now
      sorted_assets = query_assets.sort_by do |a|
                        [-a[:similarities][0], -a[:similarities][1], -a[:similarities][2]]
                      end.map { |a| a[:asset] } # rubocop:disable Style/MultilineBlockChain
      end_time = Time.now
      Rails.logger.info("sorted_assets took #{end_time - start_time} seconds")

      Result::Success.new(sorted_assets)
    end

    private

    def get_match_similarities(symbol, query_downcase)
      ticker = symbol['symbol']&.downcase
      name = symbol['name']&.downcase
      isin = symbol['isin']&.downcase
      [
        JaroWinkler.similarity(ticker.to_s, query_downcase),
        JaroWinkler.similarity(name.to_s, query_downcase),
        JaroWinkler.similarity(isin.to_s, query_downcase)
      ].sort.reverse
    end
  end
end
