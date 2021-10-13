module ExchangeApi::MapErrors
  class Probit < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        'NOT_ENOUGH_BALANCE' => Error.new('Insufficient funds', false),
        'INVALID_MARKET' => Error.new('The market doesn\'t exist', true),
        'INVALID_ARGUMENT' => Error.new('Price is out of range', true),
        'not allowed scope' => Error.new('Insufficient API keys permissions', false)
      }.freeze
    end
  end
end
