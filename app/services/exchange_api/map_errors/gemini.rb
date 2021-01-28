module ExchangeApi::MapErrors
  class Gemini < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        'Insufficient funds' => Error.new('Insufficient funds', false)
      }.freeze
    end
  end
end

