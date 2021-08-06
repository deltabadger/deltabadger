module ExchangeApi::MapErrors
  class Bitfinex < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        #'Balance insufficient!' => Error.new('Insufficient funds', false)
      }.freeze
    end
  end
end
