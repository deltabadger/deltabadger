class SubscriptionPlan < ApplicationRecord
  FREE_PLAN = 'free'.freeze
  MINI_PLAN = 'mini'.freeze
  MINI_RESEARCH_PLAN = 'mini_research'.freeze
  STANDARD_PLAN = 'standard'.freeze
  STANDARD_RESEARCH_PLAN = 'standard_research'.freeze
  PRO_PLAN = 'pro'.freeze
  LEGENDARY_PLAN = 'legendary'.freeze
  RESEARCH_PLAN = 'research'.freeze

  after_commit :reset_all_subscription_plans_cache

  has_many :subscriptions
  has_many :subscription_plan_variants, dependent: :destroy

  include PlanStats
  include PlanFeatures

  def self.free
    all_subscription_plans[FREE_PLAN]
  end

  def self.mini
    all_subscription_plans[MINI_PLAN]
  end

  def self.mini_research
    all_subscription_plans[MINI_RESEARCH_PLAN]
  end

  def self.standard
    all_subscription_plans[STANDARD_PLAN]
  end

  def self.standard_research
    all_subscription_plans[STANDARD_RESEARCH_PLAN]
  end

  def self.pro
    all_subscription_plans[PRO_PLAN]
  end

  def self.legendary
    all_subscription_plans[LEGENDARY_PLAN]
  end

  def self.research
    all_subscription_plans[RESEARCH_PLAN]
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

  def free?
    name == FREE_PLAN
  end

  def mini?
    name.in?([MINI_PLAN, MINI_RESEARCH_PLAN])
  end

  def mini_research?
    name == MINI_RESEARCH_PLAN
  end

  def standard?
    name.in?([STANDARD_PLAN, STANDARD_RESEARCH_PLAN])
  end

  def standard_research?
    name == STANDARD_RESEARCH_PLAN
  end

  def pro?
    name == PRO_PLAN
  end

  def legendary?
    name == LEGENDARY_PLAN
  end

  def research?
    name.in?([RESEARCH_PLAN, MINI_RESEARCH_PLAN, STANDARD_RESEARCH_PLAN, PRO_PLAN, LEGENDARY_PLAN])
  end

  def research_only?
    name == RESEARCH_PLAN
  end

  def max_bots
    if free? || research_only?
      1
    elsif mini? || mini_research?
      5
    elsif standard? || standard_research?
      20
    elsif pro?
      100
    elsif legendary?
      nil
    else
      raise "Unknown subscription plan: #{name}"
    end
  end

  private

  def reset_all_subscription_plans_cache
    self.class.reset_all_subscription_plans_cache
  end
end
