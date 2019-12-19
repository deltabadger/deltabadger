module ExchangeApi::MapErrors
  class Bitbay < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        'FUNDS_NOT_SUFFICIENT' => 'Your funds are insufficient',
        'OFFER_FUNDS_NOT_EXCEEDING_MINIMUMS' => 'Offer funds are not exceeding minimums',
        'PRICE_PRECISION_INVALID' => 'Price precision invalid',
      }
    end
  end
end
