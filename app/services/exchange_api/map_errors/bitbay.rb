module ExchangeApi::MapErrors
  class Bitbay < BaseService
    ERRORS = {
      'FUNDS_NOT_SUFFICIENT' => 'Your funds are insufficient',
      'OFFER_FUNDS_NOT_EXCEEDING_MINIMUMS' => 'Offer funds are not exceeding minimums',
      'PRICE_PRECISION_INVALID' => 'Price precision invalid',
    }.freeze

    def call(errors)
      errors.map { |e| ERRORS.fetch(e, e) }
    end
  end
end
