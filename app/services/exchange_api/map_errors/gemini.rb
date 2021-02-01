module ExchangeApi::MapErrors
  class Gemini < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        'InsufficientFunds' => Error.new('Insufficient funds', false),
        'InvalidSymbol' => Error.new('Pair not available', false)
      }.freeze
    end
  end
end
