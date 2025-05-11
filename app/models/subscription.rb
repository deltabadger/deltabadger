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

  scope :active, -> { where('ends_at IS NULL OR ends_at > ?', Time.current) }
  scope :by_plan_name, ->(name) { joins(:subscription_plan_variant).merge(SubscriptionPlanVariant.where(subscription_plan: SubscriptionPlan.send(name))) } # rubocop:disable Layout/LineLength

  include Nftable

  before_create :set_ends_at
  after_create_commit -> { Intercom::UpdateUserSubscriptionJob.perform_later(user) }
  after_create_commit -> { Sendgrid::UpdatePlanListJob.perform_later(user) }, unless: :first_subscription?

  def days_left
    return if ends_at.nil?

    [0, (ends_at.to_date - Date.today).to_i].max
  end

  private

  def set_ends_at
    self.ends_at ||= subscription_plan_variant&.duration&.from_now
  end

  def first_subscription?
    user.subscriptions.count == 1
  end
end
