module ExchangeApi::MapErrors
  class Base < BaseService
    def errors_mapping
      raise NotImplementedError
    end

    def call(errors)
      errors.map { |e| errors_mapping.fetch(e, e) }
    end
  end
end
