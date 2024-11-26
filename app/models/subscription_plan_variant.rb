class SubscriptionPlanVariant < ApplicationRecord
  after_commit :reset_all_subscription_plan_variants_cache

  belongs_to :subscription_plan

  validates :years, numericality: { only_integer: true, greater_than: 0 }, allow_nil: true
  validates :cost_eur, numericality: { greater_than_or_equal_to: 0 }
  validates :cost_usd, numericality: { greater_than_or_equal_to: 0 }

  delegate :name, to: :subscription_plan
  delegate :unlimited?, to: :subscription_plan
  delegate :paid?, to: :subscription_plan
  delegate :free?, to: :subscription_plan
  delegate :basic?, to: :subscription_plan
  delegate :pro?, to: :subscription_plan
  delegate :legendary?, to: :subscription_plan

  scope :years, ->(years) { where(years: years) }

  def self.free(_years_value = 1)
    all_subscription_plan_variants[[SubscriptionPlan.free.id, nil].join('-')]
  end

  def self.basic(years_value = 1)
    all_subscription_plan_variants[[SubscriptionPlan.basic.id, years_value].join('-')]
  end

  def self.pro(years_value = 1)
    all_subscription_plan_variants[[SubscriptionPlan.pro.id, years_value].join('-')]
  end

  def self.legendary(_years_value = 1)
    all_subscription_plan_variants[[SubscriptionPlan.legendary.id, nil].join('-')]
  end

  def self.variant_years(ignore_nil: true)
    y = all_subscription_plan_variants.map { |_, v| v.years }.uniq.compact.sort
    y << nil if !ignore_nil && all_subscription_plan_variants.any? { |_, v| v.years.nil? }
    y
  end

  def self.all_subscription_plan_variants
    @all_subscription_plan_variants ||= all.map { |spv| [[spv.subscription_plan_id, spv.years].join('-'), spv] }.to_h
  end

  def self.reset_all_subscription_plan_variants_cache
    @all_subscription_plan_variants = nil
  end

  def duration
    years&.years
  end

  private

  def reset_all_subscription_plan_variants_cache
    self.class.reset_all_subscription_plan_variants_cache
  end
end
