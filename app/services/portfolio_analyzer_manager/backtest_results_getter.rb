require 'utilities/time'

module PortfolioAnalyzerManager
  class BacktestResultsGetter < BaseService
    def call(portfolio)
      expires_in = Utilities::Time.seconds_to_midnight_utc.seconds + 5.minutes
      Rails.cache.fetch(portfolio.backtest_cache_key, expires_in: expires_in) do
        client = FinancialDataApiClient.new
        symbols = portfolio.assets.map(&:symbol).join(',')
        sources = portfolio.assets.map(&:source).join(',')
        allocations = portfolio.assets.map(&:allocation).join(',')
        metrics_result = client.metrics(
          symbols: symbols,
          sources: sources,
          allocations: allocations,
          benchmark: portfolio.benchmark,
          source: portfolio.benchmark_source,
          start: portfolio.backtest_start_date,
          strategy: portfolio.strategy,
          risk_free_rate: portfolio.risk_free_rate
        )
        return metrics_result if metrics_result.failure? && metrics_result.data[:status] != 400

        metrics_result
      end
    end
  end
end
