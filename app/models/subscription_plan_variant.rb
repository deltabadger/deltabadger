class SubscriptionPlanVariant < ApplicationRecord
  belongs_to :subscription_plan

  validates :years, numericality: { only_integer: true, greater_than_or_equal_to: 0 }, allow_nil: true
  validates :cost_eur, numericality: { greater_than_or_equal_to: 0 }
  validates :cost_usd, numericality: { greater_than_or_equal_to: 0 }

  delegate :name, to: :subscription_plan
  delegate :paid?, to: :subscription_plan
  delegate :free?, to: :subscription_plan
  delegate :basic?, to: :subscription_plan
  delegate :pro?, to: :subscription_plan
  delegate :legendary?, to: :subscription_plan
  delegate :features, to: :subscription_plan

  scope :years, ->(years) { where(years: years) }

  def self.all_variant_years
    all.map(&:years).uniq.compact.sort
  end

  def duration
    case years
    when 0
      1.month
    when nil
      Float::INFINITY
    else
      years.years
    end
  end
end
