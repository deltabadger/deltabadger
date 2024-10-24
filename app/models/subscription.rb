class Subscription < ApplicationRecord
  belongs_to :subscription_plan_variant
  belongs_to :user

  scope :active, -> { where('end_time > ?', Time.current) }
  scope :by_plan_name, ->(name) { joins(:subscription_plan_variant).merge(SubscriptionPlanVariant.where(subscription_plan: SubscriptionPlan.send(name))) } # rubocop:disable Layout/LineLength

  delegate :name, to: :subscription_plan_variant
  delegate :display_name, to: :subscription_plan_variant
  delegate :unlimited?, to: :subscription_plan_variant

  include Nftable

  def days_left
    (end_time.to_date - Date.today).to_i
  end
end
