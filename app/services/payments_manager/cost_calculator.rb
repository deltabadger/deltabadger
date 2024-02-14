module PaymentsManager
  class CostCalculator
    attr_reader :base_price, :vat, :flat_discount, :discount_percent, :commission_percent, :early_bird_discount

    def initialize(base_price:, vat:, flat_discount: 0, discount_percent: 0, commission_percent: 0, early_bird_discount: 0)
      @base_price = to_bigdecimal(base_price)
      @vat = to_bigdecimal(vat)
      @flat_discount = to_bigdecimal(flat_discount)
      @discount_percent = to_bigdecimal(discount_percent)
      @commission_percent = to_bigdecimal(commission_percent)
      @early_bird_discount = to_bigdecimal(early_bird_discount)
    end

    def base_price_with_vat
      @base_price_with_vat ||= round_down(base_price * vat_multiplier)
    end

    def flat_discounted_price
      @flat_discounted_price = base_price - flat_discount - early_bird_discount
    end

    def discount_percent_amount
      @discount_percent_amount = flat_discounted_price - discounted_price
    end

    def discounted_price
      @discounted_price = round_down(flat_discounted_price * discount_multiplier)
    end

    def total_vat
      @total_vat = total_price - discounted_price
    end

    def total_price
      @total_price ||= round_down(discounted_price * vat_multiplier)
    end

    def commission
      @commission ||= round_down((base_price - flat_discount - early_bird_discount) * commission_percent)
    end

    def crypto_commission(crypto_total_price:)
      crypto_total_price = to_crypto_bigdecimal(crypto_total_price)
      crypto_without_vat = crypto_total_price / vat_multiplier
      crypto_base_price = crypto_without_vat / discount_multiplier
      round_crypto_down(crypto_base_price * commission_percent)
    end

    private

    def vat_multiplier
      1 + vat
    end

    def discount_multiplier
      1 - discount_percent
    end

    def to_crypto_bigdecimal(num)
      BigDecimal(format('%0.08f', num))
    end

    def to_bigdecimal(num)
      BigDecimal(format('%0.02f', num))
    end

    def round_down(num)
      num.round(2)
    end

    def round_crypto_down(num)
      num.round(8, BigDecimal::ROUND_DOWN)
    end
  end
end
