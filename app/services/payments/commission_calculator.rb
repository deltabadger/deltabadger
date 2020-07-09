module Payments
  class CommissionCalculator
    def call(base_price:, vat:, discount:, commission_percent:, crypto_total_price:)
      base_price = to_bigdecimal(base_price)
      vat = to_bigdecimal(vat)
      discount = to_bigdecimal(discount)
      commission_percent = to_bigdecimal(commission_percent)
      crypto_total_price = BigDecimal(crypto_total_price)
      crypto_base_price = crypto_total_price / (1 + vat) / (1 - discount)
      crypto_commission = crypto_base_price * commission_percent
      commission = base_price * commission_percent

      {
        commission: commission,
        crypto_commission: crypto_commission
      }
    end

    def to_bigdecimal(num)
      BigDecimal(format('%0.02f', num))
    end
  end
end
