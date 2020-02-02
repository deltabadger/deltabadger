module ExchangeApi::MapErrors
  class Kraken < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        'EGeneral:Permission denied' => 'Insufficient permissions or unverified account.',
        'EOrder:Insufficient funds' => 'Insufficient funds',
        'EGeneral:Invalid arguments:volume' => 'Offer funds are not exceeding minimums'
      }
    end
  end
end
