module ExchangeApi::MapErrors
  class Kucoin < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        'Balance insufficient!' => Error.new('Insufficient funds', false),
        'too many request' => Error.new('API reached limit of requests. We\'ll try again.', true)
      }.freeze
    end
  end
end
