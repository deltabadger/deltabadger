class SubscriptionPlanVariant < ApplicationRecord
  belongs_to :subscription_plan

  validates :years, numericality: { only_integer: true, greater_than: 0 }, allow_nil: true
  validates :cost_eur, numericality: { greater_than_or_equal_to: 0 }
  validates :cost_usd, numericality: { greater_than_or_equal_to: 0 }

  delegate :name, to: :subscription_plan
  delegate :unlimited?, to: :subscription_plan
  delegate :paid?, to: :subscription_plan

  scope :years, ->(years) { where(years: years) }

  def self.free
    find_by!(subscription_plan: SubscriptionPlan.free)
  end

  def self.basic(variant_years = 1)
    find_by!(subscription_plan: SubscriptionPlan.basic, years: variant_years)
  end

  def self.pro(variant_years = 1)
    find_by!(subscription_plan: SubscriptionPlan.pro, years: variant_years)
  end

  def self.legendary
    find_by!(subscription_plan: SubscriptionPlan.legendary)
  end

  def duration
    years&.years
  end
end
