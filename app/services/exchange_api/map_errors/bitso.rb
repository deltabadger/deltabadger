module ExchangeApi::MapErrors
  class Bitso < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        '0343' => Error.new('Insufficient funds', false)
      }.freeze
    end
  end
end
