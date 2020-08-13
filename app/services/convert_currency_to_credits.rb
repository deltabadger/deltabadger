class ConvertCurrencyToCredits < BaseService
  def call(amount:, currency:)
    conversion_rate = ConversionRate.find_by(currency: currency.downcase)&.rate || BigDecimal(1)
    amount / conversion_rate
  end
end
