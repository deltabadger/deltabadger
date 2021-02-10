module ExchangeApi::MapErrors
  class Ftx < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        'Not enough balances' => Error.new('Insufficient funds', false)
      }.freeze
    end
  end
end
