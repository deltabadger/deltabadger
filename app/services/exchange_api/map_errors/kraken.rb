module ExchangeApi::MapErrors
  class Kraken < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        'EGeneral:Permission denied' => 'Permission denied. Check API settings.',
      }
    end
  end
end
