class CostPresenter
  attr_reader :cost_data

  def initialize(cost_data)
    @cost_data = cost_data
  end

  def base_price
    format_price(cost_data[:base_price])
  end

  def vat
    format_price(cost_data[:vat])
  end

  def flat_discount
    format_price(cost_data[:flat_discount])
  end

  def discount_percent_amount
    format_price(cost_data[:discount_percent_amount])
  end

  def vat_integer
    (100 * cost_data[:vat]).to_i.to_s
  end

  def flat_discounted_price
    format_price(cost_data[:flat_discounted_price] -
                   cost_data[:discount_percent_amount])
  end

  def total_vat
    format_price(cost_data[:total_vat])
  end

  def total_price
    format_price(cost_data[:total_price])
  end

  def early_bird_discount
    format_price(cost_data[:early_bird_discount])
  end

  private

  def format_price(price)
    format('%0.02f', price)
  end
end
