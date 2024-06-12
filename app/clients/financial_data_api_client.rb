class FinancialDataApiClient < ApplicationClient
  URL = ENV.fetch('FINANCIAL_DATA_API_URL').freeze

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  def metrics(
    symbols: nil,
    sources: nil,
    allocations: nil,
    benchmark: nil,
    source: nil,
    start: nil,
    strategy: nil,
    risk_free_rate: nil,
    start_balance: 10_000
  )
    with_rescue do
      response = self.class.connection.get do |req|
        req.url 'metrics'
        req.params = {
          symbols: symbols,
          sources: sources,
          allocations: allocations,
          benchmark: benchmark,
          source: source,
          start: start,
          strategy: strategy,
          risk_free_rate: risk_free_rate,
          'start-balance': start_balance
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  def symbols(source)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url 'symbols'
        req.params = {
          source: source
        }
      end
      Result::Success.new(response.body)
    end
  end

  def smart_allocations(
    symbols: nil,
    sources: nil,
    start: nil,
    strategy: nil,
    risk_free_rate: nil
  )
    with_rescue do
      response = self.class.connection.get do |req|
        req.url 'smart-allocations'
        req.params = {
          symbols: symbols,
          sources: sources,
          start: start,
          strategy: strategy,
          risk_free_rate: risk_free_rate
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  def time_series(
    symbol:,
    timeframe:,
    source: nil,
    limit: nil,
    start: nil
  )
    with_rescue do
      response = self.class.connection.get do |req|
        req.url 'time-series'
        req.params = {
          symbol: symbol,
          timeframe: timeframe,
          source: source,
          limit: limit,
          start: start
        }.compact
      end
      Result::Success.new(response.body)
    end
  end
end
