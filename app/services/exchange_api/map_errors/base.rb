module ExchangeApi::MapErrors
  class Base < BaseService
    Error = Struct.new(:message, :recoverable)

    def errors_mapping
      raise NotImplementedError
    end

    def call(errors)
      errors.reduce(Error.new([], true)) do |joined, error|
        mapped_error = errors_mapping.fetch(error, Error.new(error, true))
        Error.new(
          joined.message + [mapped_error.message],
          joined.recoverable && mapped_error.recoverable
        )
      end
    end
  end
end
