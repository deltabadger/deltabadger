module ExchangeApi::MapErrors
  class Kraken < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        'EAPI:Invalid nonce' => Error.new('A network inconsistency has occurred. Please wait a few seconds and try again', true),
        'EFunding:Invalid amount' => Error.new('Funds are not exceeding minimums', true),
        'EFunding:Unknown withdraw key' => Error.new('Provided address label does not exist', true),
        'EGeneral:Invalid arguments:volume' => Error.new('Offer funds are not exceeding minimums', true),
        'EGeneral:Permission denied' => Error.new('Insufficient permissions or unverified account.', false),
        'EGeneral:Temporary lockout' => Error.new('Funds are not exceeding minimums', true),
        'EOrder:Insufficient funds' => Error.new('Insufficient funds', true),
        'EOrder:Orders limit exceeded' => Error.new('Action limit was exceeded', true),
        'EService:Market in cancel_only mode' => Error.new('Funds are not exceeding minimums', true),
        'EService:Unavailable' => Error.new('The exchange is currently unavailable', true),
        'Out of funds' => Error.new('Insufficient funds', true)
      }
    end
  end
end
