class SubscriptionPlanVariant < ApplicationRecord
  belongs_to :subscription_plan

  validates :days, numericality: { only_integer: true, greater_than_or_equal_to: 0 }, allow_nil: true
  validates :cost_eur, numericality: { greater_than_or_equal_to: 0 }, allow_nil: true
  validates :cost_usd, numericality: { greater_than_or_equal_to: 0 }, allow_nil: true

  delegate :name, to: :subscription_plan
  delegate :paid?, to: :subscription_plan
  delegate :free?, to: :subscription_plan
  delegate :mini?, to: :subscription_plan
  delegate :mini_research?, to: :subscription_plan
  delegate :standard?, to: :subscription_plan
  delegate :standard_research?, to: :subscription_plan
  delegate :pro?, to: :subscription_plan
  delegate :legendary?, to: :subscription_plan
  delegate :research?, to: :subscription_plan
  delegate :research_only?, to: :subscription_plan
  delegate :features, to: :subscription_plan
  delegate :max_bots, to: :subscription_plan

  scope :days, ->(days) { where(days: days) }

  def self.all_variant_days
    all.map(&:days).uniq.compact.sort
  end

  def duration
    case days
    when 7
      1.week
    when 30
      1.month
    when 365
      1.year
    when 1460
      4.years
    when nil
      Float::INFINITY
    else
      raise "Unknown exact duration: #{days}. Please update SubscriptionPlanVariant#duration."
    end
  end
end
