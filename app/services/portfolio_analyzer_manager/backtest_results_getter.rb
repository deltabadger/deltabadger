require 'utilities/time'

module PortfolioAnalyzerManager
  class BacktestResultsGetter < BaseService
    def call(portfolio)
      expires_in = Utilities::Time.seconds_to_midnight_utc.seconds + 5.minutes
      Rails.cache.fetch(portfolio.backtest_cache_key, expires_in: expires_in) do
        client = FinancialDataApiClient.new
        symbols = portfolio.assets.map(&:api_id).join(',')
        metrics_result = client.metrics(
          symbols: symbols,
          allocations: allocations(portfolio),
          benchmark: portfolio.benchmark,
          start: portfolio.backtest_start_date,
          strategy: portfolio.strategy,
          risk_free_rate: portfolio.risk_free_rate
        )
        return metrics_result if metrics_result.failure? && metrics_result.data[:status] != 400

        metrics_result
      end
    end

    private

    def allocations(portfolio)
      if portfolio.smart_allocation_on?
        portfolio.assets.map { |asset| portfolio.smart_allocations[portfolio.risk_level_int][asset.api_id] }.join(',')
      else
        portfolio.assets.map(&:allocation).join(',')
      end
    end
  end
end
