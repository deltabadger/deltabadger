class PortfolioAnalyzerController < ApplicationController
  def index
    client = FinancialDataApiClient.new

    # allocations_result = client.smart_allocations('BTC/USDT,ETH/USDT', '2021-01-01', 'fixed')
    # @allocations = allocations_result.data
    # puts allocations_result.data

    metrics_result = client.metrics('BTC/USDT,ETH/USDT', '0.6819, 0.3181', '^GSPC', '2021-01-01', 'fixed')
    @metrics = metrics_result.data
    # puts metrics_result.data

    @data_labels = metrics_result.data['timeSeries']['labels']
    @data_series = metrics_result.data['timeSeries']['series']
  end
end
