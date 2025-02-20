class Subscription < ApplicationRecord
  belongs_to :user
  belongs_to :subscription_plan_variant

  validates :user, :subscription_plan_variant, presence: true

  delegate :name, to: :subscription_plan_variant
  delegate :unlimited?, to: :subscription_plan_variant
  delegate :paid?, to: :subscription_plan_variant
  delegate :free?, to: :subscription_plan_variant
  delegate :basic?, to: :subscription_plan_variant
  delegate :pro?, to: :subscription_plan_variant
  delegate :legendary?, to: :subscription_plan_variant
  delegate :features, to: :subscription_plan_variant

  scope :active, -> { where('end_time IS NULL OR end_time > ?', Time.current) }
  scope :by_plan_name, ->(name) { joins(:subscription_plan_variant).merge(SubscriptionPlanVariant.where(subscription_plan: SubscriptionPlan.send(name))) } # rubocop:disable Layout/LineLength

  include Nftable

  def initialize(attributes = {})
    super
    self.end_time ||= subscription_plan_variant&.duration&.from_now
  end

  def days_left
    return if end_time.nil?

    [0, (end_time.to_date - Date.today).to_i].max
  end
end
