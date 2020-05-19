module ExchangeApi::MapErrors
  class Bitbay < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        'FUNDS_NOT_SUFFICIENT' => 'Funds not sufficient',
        'OFFER_FUNDS_NOT_EXCEEDING_MINIMUMS' => 'Funds not exceeding minimums',
        'PRICE_PRECISION_INVALID' => 'Price precision invalid',
        'PERMISSIONS_NOT_SUFFICIENT' => 'API keys permissions not sufficient'
      }
    end
  end
end
