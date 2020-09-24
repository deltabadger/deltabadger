require 'result'

module ExchangeApi
  module Clients
    class BaseClient
      RECOVERABLE = { data: { recoverable: true }.freeze }.freeze

      protected

      def error_to_failure(error)
        mapped_error = @map_errors.call(error)
        Result::Failure.new(
          *mapped_error.message, data: { recoverable: mapped_error.recoverable }
        )
      end
    end
  end
end
