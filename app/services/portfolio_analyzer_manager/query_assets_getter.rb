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
          ticker: ticker(symbol),
          name: symbol['name'],
          portfolio_id: portfolio.id,
          category: category(symbol),
          color: symbol['color']
        )
      end.compact
      Result::Success.new(query_assets)
    end

    private

    def get_remote_symbols_info
      expires_in = Utilities::Time.seconds_to_midnight_utc.seconds
      Rails.cache.fetch('symbols', expires_in: expires_in) do
        client = FinancialDataApiClient.new
        symbols_result = client.symbols('all')
        return [] if symbols_result.failure?

        symbols_result.data
      end
    end

    def ticker(symbol)
      symbol['source'] == 'binance' ? symbol['symbol'].gsub(/USDT$/, '').upcase : symbol['symbol'].upcase
    end

    def category(symbol)
      custom_type = CUSTOM_CATEGORIES[ticker(symbol).to_sym]
      return custom_type if custom_type

      symbol['source'] == 'yfinance' ? 'stock' : 'crypto'
    end

    def asset_already_in_portfolio?(portfolio, symbol)
      portfolio_tickers = portfolio.assets.map(&:ticker)
      portfolio_tickers.include?(ticker(symbol))
    end

    def matches_query?(symbol, query)
      query = query.upcase
      name = symbol['name']&.upcase
      ticker(symbol).upcase.include?(query) || name&.include?(query)
    end
  end
end
