module ExchangeApi::MapErrors
  class Ftx < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        'Not enough balances' => Error.new('Insufficient funds', false),
        'Not approved to trade this product' => Error.new('Pair not available', false)
      }.freeze
    end
  end
end
