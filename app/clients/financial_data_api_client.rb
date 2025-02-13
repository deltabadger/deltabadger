class FinancialDataApiClient < ApplicationClient
  URL = ENV.fetch('FINANCIAL_DATA_API_URL').freeze
  KEY = ENV.fetch('FINANCIAL_DATA_API_KEY').freeze

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.headers = {
        'X-API-Key': KEY
      }
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :info
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  def metrics(
    symbols: nil,
    allocations: nil,
    benchmark: nil,
    start: nil,
    strategy: nil,
    risk_free_rate: nil,
    start_balance: 10_000
  )
    with_rescue do
      response = self.class.connection.get do |req|
        req.url 'v3/metrics'
        req.params = {
          symbols: symbols,
          allocations: allocations,
          benchmark: benchmark,
          start: start,
          strategy: strategy,
          risk_free_rate: risk_free_rate,
          start_balance: start_balance,
          include_dividends: true
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  def symbols(source)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url 'v3/symbols'
        req.params = {
          source: source
        }.compact
        req.options.read_timeout = 15
      end
      Result::Success.new(response.body)
    end
  end

  def smart_allocations(
    symbols: nil,
    start: nil,
    strategy: nil,
    risk_free_rate: nil
  )
    with_rescue do
      response = self.class.connection.get do |req|
        req.url 'v3/smart-allocations'
        req.params = {
          symbols: symbols,
          start: start,
          strategy: strategy,
          risk_free_rate: risk_free_rate,
          include_dividends: true
        }.compact
      end
      Result::Success.new(response.body)
    end
  end

  def time_series(
    symbol:,
    timeframe:,
    limit: nil,
    start: nil
  )
    # end
    # response.body
    with_rescue do
      response = self.class.connection.get do |req|
        req.url 'v3/time-series'
        req.params = {
          symbol: symbol,
          timeframe: timeframe,
          limit: limit,
          start: start,
          include_dividends: true
        }.compact
      end
      Result::Success.new(response.body)
    end
  end
end
