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

  def metrics(symbols, allocations, benchmark, start, strategy, start_balance = 1)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url 'metrics'
        req.params = {
          symbols: symbols,
          allocations: allocations,
          benchmark: benchmark,
          start: start,
          strategy: strategy,
          'start-balance': start_balance
        }
      end
      Result::Success.new(response.body)
    end
  end

  def smart_allocations(symbols, start, strategy)
    with_rescue do
      response = self.class.connection.get do |req|
        req.url 'smart-allocations'
        req.params = {
          symbols: symbols,
          start: start,
          strategy: strategy
        }
      end
      Result::Success.new(response.body)
    end
  end
end
