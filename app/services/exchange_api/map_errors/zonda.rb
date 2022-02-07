module ExchangeApi::MapErrors
  class Zonda < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        'FUNDS_NOT_SUFFICIENT' => Error.new('Funds not sufficient', false),
        'OFFER_FUNDS_NOT_EXCEEDING_MINIMUMS' => Error.new('Funds not exceeding minimums', true),
        'PRICE_PRECISION_INVALID' => Error.new('Price precision invalid', false),
        'PERMISSIONS_NOT_SUFFICIENT' => Error.new('API keys permissions not sufficient', false),
        'RESPONSE_TIMEOUT' => Error.new('Response time was exceeded', true),
        'TIMEOUT' => Error.new('Response time was exceeded', true),
        'ACTION_LIMIT_EXCEEDED' => Error.new('Action limit was exceeded', true),
        'UNDER_MAINTENANCE' => Error.new('The exchange is currently under maintenance', true)
      }.freeze
    end
  end
end
