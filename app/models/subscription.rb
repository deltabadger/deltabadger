class Subscription < ApplicationRecord
  belongs_to :user
  belongs_to :subscription_plan_variant

  validates :user, :subscription_plan_variant, presence: true

  delegate :name, to: :subscription_plan_variant
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
  after_create_commit :sync_sendgrid, unless: :first_subscription?
  after_create_commit :sync_intercom
  after_update_commit :sync_sendgrid, :sync_intercom, if: :saved_change_to_ends_at?

  # make subscription immutable, if we need to:
  # - upgrade: create a new subscription with ends_at set to nil || a date in the future
  # - downgrade: modify ends_at to a date in the past
  # - extend duration: modify ends_at to a date in the future
  attr_readonly :user, :subscription_plan_variant

  def days_left
    return if ends_at.nil?

    [0, (ends_at.to_date - Date.today).to_i].max
  end

  private

  def set_ends_at
    self.ends_at ||= if subscription_plan_variant.duration == Float::INFINITY
                       nil
                     else
                       subscription_plan_variant.duration.from_now
                     end
  end

  def first_subscription?
    user.subscriptions.count == 1
  end

  def sync_sendgrid
    Sendgrid::SyncPlanListJob.perform_later(user)
    return unless ends_at.present?

    Sendgrid::SyncPlanListJob.set(wait_until: ends_at).perform_later(user)
  end

  def sync_intercom
    Intercom::UpdateUserSubscriptionJob.perform_later(user)
    return unless ends_at.present?

    Intercom::UpdateUserSubscriptionJob.set(wait_until: ends_at).perform_later(user)
  end
end
