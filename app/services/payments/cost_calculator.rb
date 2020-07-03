module Payments
  class CostCalculator < BaseService
    def call(base_price:, vat: 0, discount: 0)
      base_price = to_bigdecimal(base_price)
      vat = to_bigdecimal(vat)
      discount = to_bigdecimal(discount)

      {
        base_price: base_price,
        vat: vat,
        discount: discount,
        price_with_vat: base_price * (1 + vat),
        total_price: (1 - discount) * base_price * (1 + vat)
      }
    end

    def to_bigdecimal(num)
      BigDecimal(format('%0.02f', num))
    end
  end
end
