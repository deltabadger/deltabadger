class SubscriptionPlan < ApplicationRecord
  FREE_PLAN = 'free'.freeze
  STANDARD_PLAN = 'standard'.freeze
  PRO_PLAN = 'pro'.freeze
  LEGENDARY_PLAN = 'legendary'.freeze

  has_many :subscriptions
  has_many :subscription_plan_variants, dependent: :destroy

  validates :credits, numericality: { only_integer: true, greater_than: 0 }

  def duration
    years.years
  end

  def display_name
    I18n.t("subscriptions.#{name}")
  end

  def free
    find_by_name!(FREE_PLAN)
  end

  def standard
    find_by_name!(STANDARD_PLAN)
  end

  def pro
    find_by_name!(PRO_PLAN)
  end

  def legendary
    find_by_name!(LEGENDARY_PLAN)
  end

  private

  def plan_cache
    @plan_cache ||= self.class.all.map { |sp| [sp.name, sp] }.to_h
  end

  def find_by_name!(name)
    plan_cache.fetch(name)
  end
end
