class SubscriptionPlan < ApplicationRecord
  FREE_PLAN = 'free'.freeze
  STANDARD_PLAN = 'standard'.freeze
  PRO_PLAN = 'pro'.freeze
  LEGENDARY_PLAN = 'legendary'.freeze
  LEGENDARY_PLAN_TOTAL_SUPPLY = 1000

  has_many :subscriptions
  has_many :subscription_plan_variants, dependent: :destroy

  validates :credits, numericality: { only_integer: true, greater_than: 0 }

  include PlanStats

  def self.free
    find_by!(name: FREE_PLAN)
  end

  def self.standard
    find_by!(name: STANDARD_PLAN)
  end

  def self.pro
    find_by!(name: PRO_PLAN)
  end

  def self.legendary
    find_by!(name: LEGENDARY_PLAN)
  end

  def duration
    years.years
  end

  def display_name
    I18n.t("subscriptions.#{name}")
  end
end
