module ExchangeApi::MapErrors
  class CoinbasePro < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        'Insufficient funds' => Error.new('Insufficient funds', false),
        'Trading pair not available' => Error.new('Pair not available', false)
      }.freeze
    end
  end
end
