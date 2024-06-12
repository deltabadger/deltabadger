module PortfolioAnalyzerManager
  class FinancialDataApiErrorParser < BaseService
    def call(result)
      return unless result_failure_with_status_400?(result)

      parse_api_error(result)
    end

    private

    def result_failure_with_status_400?(result)
      result.failure? && result.data.is_a?(Hash) && result.data[:status] == 400
    end

    def parse_api_error(result)
      first_error = JSON.parse(result.errors.first)
      first_error['detail']
    rescue StandardError
      nil
    end
  end
end
