class PortfolioAnalyzerController < ApplicationController
  def index
    client = FinancialDataApiClient.new

    allocations_result = client.smart_allocations('BTC/USDT,ETH/USDT', '2021-01-01', 'fixed')
    return if allocations_result.failure?

    allocations = allocations_result.data[2].join(',')
    puts allocations

    metrics_result = client.metrics('BTC/USDT,ETH/USDT', allocations, '^GSPC', '2021-01-01', 'fixed')
    return if metrics_result.failure?

    @metrics = metrics_result.data['metrics']

    @data_labels = metrics_result.data['timeSeries']['labels']
    @data_series = metrics_result.data['timeSeries']['series']
  end
end
