module ExchangeApi::MapErrors
  class Kraken < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        'EGeneral:Permission denied' => Error.new('Insufficient permissions or unverified account.', false),
        'EOrder:Insufficient funds' => Error.new('Insufficient funds', false),
        'EGeneral:Invalid arguments:volume' => Error.new('Offer funds are not exceeding minimums', true),
        'EService:Unavailable' => Error.new('The exchange is currently unavailable', true),
        'EOrder:Orders limit exceeded' => Error.new('Action limit was exceeded', true),
        'EAPI:Invalid nonce' => Error.new('A network inconsistency has occurred. Please wait a few seconds and try again', true),
        'Out of funds' => Error.new('Insufficient funds', false),
        'EFunding:Unknown withdraw key' => Error.new('Provided address label does not exist', false),
        'EFunding:Invalid amount' => Error.new('Funds are not exceeding minimums', true)
      }
    end
  end
end
