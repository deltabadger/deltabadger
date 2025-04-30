module PortfolioAnalyzerManager
  class BacktestResultsGetter < BaseService
    def call(portfolio, custom_start_date: nil)
      expires_in = Utilities::Time.seconds_to_midnight_utc.seconds + 5.minutes
      start_date = custom_start_date || portfolio.backtest_start_date
      Rails.cache.fetch(portfolio.backtest_cache_key(custom_start_date: start_date), expires_in: expires_in) do
        client = FinancialDataApiClient.new
        symbols = portfolio.portfolio_assets.map(&:api_id).join(',')
        allocations = portfolio.portfolio_assets.map(&:effective_allocation).join(',')
        metrics_result = client.metrics(
          symbols: symbols,
          allocations: allocations,
          benchmark: portfolio.benchmark,
          start: start_date.to_s,
          strategy: portfolio.strategy,
          risk_free_rate: portfolio.risk_free_rate
        )
        return metrics_result if metrics_result.failure? && metrics_result.data[:status] != 400

        metrics_result
      end
    end
  end
end
