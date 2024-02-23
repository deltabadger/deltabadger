class CostPresenter
  attr_reader :cost_calculator

  def initialize(cost_calculator)
    @cost_calculator = cost_calculator
  end

  def base_price
    format_price(cost_calculator.base_price)
  end

  def vat
    format_price(cost_calculator.vat)
  end

  def flat_discount
    format_price(cost_calculator.flat_discount)
  end

  def discount_percent_amount
    format_price(cost_calculator.discount_percent_amount)
  end

  def vat_integer
    (100 * cost_calculator.vat).to_i.to_s
  end

  def flat_discounted_price
    format_price(cost_calculator.flat_discounted_price -
                   cost_calculator.discount_percent_amount)
  end

  def total_vat
    format_price(cost_calculator.total_vat)
  end

  def total_price
    format_price(cost_calculator.total_price)
  end

  def early_bird_discount
    format_price(cost_calculator.early_bird_discount)
  end

  private

  def format_price(price)
    format('%0.02f', price)
  end
end
