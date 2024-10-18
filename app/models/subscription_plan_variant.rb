class SubscriptionPlanVariant < ApplicationRecord
  belongs_to :subscription_plan

  validates :years, numericality: { only_integer: true, greater_than: 0 }
  validates :cost_eur, numericality: { greater_than_or_equal_to: 0 }
  validates :cost_usd, numericality: { greater_than_or_equal_to: 0 }
end
