module ExchangeApi::MapErrors
  class Bitstamp < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        # '0343' => Error.new('Insufficient funds', false),
        # '0403' => Error.new('Offer funds are not exceeding minimums', true),
        # '0405' => Error.new('Offer funds are not exceeding minimums', true)
      }.freeze
    end
  end
end
