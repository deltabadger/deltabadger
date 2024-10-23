class Subscription < ApplicationRecord
  belongs_to :subscription_plan
  belongs_to :user

  scope :active, -> { where('end_time > ?', Time.current) }
  scope :by_plan_name, ->(name) { joins(:subscription_plan).merge(SubscriptionPlan.where(name: name)) }

  delegate :name, to: :subscription_plan
  delegate :display_name, to: :subscription_plan
  delegate :unlimited?, to: :subscription_plan

  include Nftable
end
