module PortfolioAnalyzerManager
  class SmartAllocationsGetter < BaseService
    def call(portfolio)
      expires_in = Utilities::Time.seconds_to_midnight_utc.seconds + 5.minutes
      Rails.cache.fetch(portfolio.smart_allocations_cache_key, expires_in: expires_in) do
        client = FinancialDataApiClient.new
        symbols = portfolio.assets.map(&:api_id).join(',')
        allocations_result = client.smart_allocations(
          symbols: symbols,
          start: portfolio.backtest_start_date.to_s,
          strategy: portfolio.strategy,
          risk_free_rate: portfolio.risk_free_rate
        )
        if allocations_result.failure?
          return allocations_result unless allocations_result.data[:status] == 400

          allocations_result
        else
          Result::Success.new(format_allocations(allocations_result.data))
        end
      end
    end

    private

    def format_allocations(allocations)
      allocations.map { |r| r.transform_keys { |s| s.gsub(%r{/USDT$}, '') } }
    end
  end
end
