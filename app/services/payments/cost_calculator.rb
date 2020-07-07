module Payments
  class CostCalculator < BaseService
    def call(base_price:, vat: 0, discount: 0)
      base_price = to_bigdecimal(base_price)
      vat = to_bigdecimal(vat)
      discount = to_bigdecimal(discount)

      price_with_vat = base_price * (1 + vat)
      total_price = price_with_vat * (1 - discount)

      {
        base_price: base_price,
        vat: vat,
        discount: discount,
        price_with_vat: price_with_vat,
        total_price: total_price
      }
    end

    private

    def to_bigdecimal(num)
      BigDecimal(format('%0.02f', num))
    end
  end
end
