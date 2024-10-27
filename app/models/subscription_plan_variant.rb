class SubscriptionPlanVariant < ApplicationRecord
  belongs_to :subscription_plan

  validates :years, numericality: { only_integer: true, greater_than: 0 }, allow_nil: true
  validates :cost_eur, numericality: { greater_than_or_equal_to: 0 }
  validates :cost_usd, numericality: { greater_than_or_equal_to: 0 }

  delegate :name, to: :subscription_plan
  delegate :unlimited?, to: :subscription_plan
  delegate :paid?, to: :subscription_plan

  scope :years, ->(years) { where(years: years) }

  def self.free(_years_value = 1)
    find_by!(subscription_plan: SubscriptionPlan.free)
  end

  def self.basic(years_value = 1)
    find_by!(subscription_plan: SubscriptionPlan.basic, years: years_value)
  end

  def self.pro(years_value = 1)
    find_by!(subscription_plan: SubscriptionPlan.pro, years: years_value)
  end

  def self.legendary(_years_value = 1)
    find_by!(subscription_plan: SubscriptionPlan.legendary)
  end

  def self.variant_years(ignore_nil: true)
    ignore_nil ? where.not(years: nil).pluck(:years).uniq : pluck(:years).uniq
  end

  def duration
    years&.years
  end
end
