module PaymentsManager
  class CostCalculator < BaseService
    def call(
      base_price:,
      vat:,
      flat_discount: 0,
      discount_percent: 0,
      commission_percent: 0,
      early_bird_discount: 0
    )
      @base_price = to_bigdecimal(base_price)
      @vat = to_bigdecimal(vat)
      @flat_discount = to_bigdecimal(flat_discount)
      @discount_percent = to_bigdecimal(discount_percent)
      @commission_percent = to_bigdecimal(commission_percent)
      @early_bird_discount = to_bigdecimal(early_bird_discount)
      begin
        Result::Success.new(calculate_cost_data)
      rescue StandardError => e
        Result::Failure.new(e.message)
      end
    end

    private

    def calculate_cost_data
      {
        base_price: @base_price,
        vat: @vat,
        flat_discount: @flat_discount,
        discount_percent: @discount_percent,
        commission_percent: @commission_percent,
        early_bird_discount: @early_bird_discount,
        flat_discounted_price: flat_discounted_price,
        discount_percent_amount: discount_percent_amount,
        total_vat: total_vat,
        total_price: total_price,
        commission: commission
      }
    end

    def flat_discounted_price
      @flat_discounted_price ||= @base_price - @flat_discount - @early_bird_discount
    end

    def discount_percent_amount
      (flat_discounted_price * @discount_percent).round(2)
    end

    def total_vat
      total_price - discounted_price
    end

    def total_price
      @total_price ||= (discounted_price * (1 + @vat)).round(2)
    end

    def commission
      ((@base_price - @flat_discount - @early_bird_discount) * @commission_percent).round(2)
    end

    def discounted_price
      (flat_discounted_price * (1 - @discount_percent)).round(2)
    end

    # FIXME: use generic to_bigdecimal method (helper?)
    def to_bigdecimal(num, precision: 2)
      BigDecimal(format("%0.0#{precision}f", num))
    end

    def round_down(num)
      num.round(2)
    end
  end
end
