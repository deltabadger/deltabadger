class ConvertCurrencyToCredits < BaseService
  CONVERSION_RATES = {
    PLN: 0.25
  }.freeze

  def call(amount:, currency:)
    conversion_rate = CONVERSION_RATES.fetch(currency.to_sym, 1)
    byebug
    amount * conversion_rate
  end
end
