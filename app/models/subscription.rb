class Subscription < ApplicationRecord
  belongs_to :user
  belongs_to :subscription_plan_variant

  validates :user, :subscription_plan_variant, :end_time, presence: true

  delegate :name, to: :subscription_plan_variant
  delegate :unlimited?, to: :subscription_plan_variant

  scope :active, -> { where('end_time IS NULL OR end_time > ?', Time.current) }
  scope :by_plan_name, ->(name) { joins(:subscription_plan_variant).merge(SubscriptionPlanVariant.where(subscription_plan: SubscriptionPlan.send(name))) } # rubocop:disable Layout/LineLength

  include Nftable

  def initialize(attributes = {})
    super
    self.end_time ||= subscription_plan_variant&.years&.years&.from_now
  end

  def days_left
    return if end_time.nil?

    (end_time.to_date - Date.today).to_i
  end
end
