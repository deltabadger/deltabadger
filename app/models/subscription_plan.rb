class SubscriptionPlan < ApplicationRecord
  validates :years, numericality: { only_integer: true, greater_than: 0 }
  validates :cost_eu, numericality: { greater_than_or_equal_to: 0 }
  validates :cost_other, numericality: { greater_than_or_equal_to: 0 }
  validates :credits, numericality: { only_integer: true, greater_than: 0 }

  def duration
    years.to_i.years
  end

  def display_name
    name.capitalize
  end
end
