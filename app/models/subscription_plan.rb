class SubscriptionPlan < ApplicationRecord
  FREE_PLAN = 'free'.freeze
  BASIC_PLAN = 'basic'.freeze
  PRO_PLAN = 'pro'.freeze
  LEGENDARY_PLAN = 'legendary'.freeze

  after_commit :reset_all_subscription_plans_cache

  has_many :subscriptions
  has_many :subscription_plan_variants, dependent: :destroy

  validates :credits, numericality: { only_integer: true, greater_than: 0 }

  include PlanStats

  def self.free
    all_subscription_plans[FREE_PLAN]
  end

  def self.basic
    all_subscription_plans[BASIC_PLAN]
  end

  def self.pro
    all_subscription_plans[PRO_PLAN]
  end

  def self.legendary
    all_subscription_plans[LEGENDARY_PLAN]
  end

  def self.all_subscription_plans
    @all_subscription_plans ||= all.map { |sp| [sp.name, sp] }.to_h
  end

  def self.reset_all_subscription_plans_cache
    @all_subscription_plans = nil
  end

  def paid?
    name != FREE_PLAN
  end

  private

  def reset_all_subscription_plans_cache
    self.class.reset_all_subscription_plans_cache
  end
end
