class Subscription < ApplicationRecord
  belongs_to :user
  belongs_to :subscription_plan_variant

  validates :user, :subscription_plan_variant, presence: true

  delegate :name, to: :subscription_plan_variant
  delegate :paid?, to: :subscription_plan_variant
  delegate :free?, to: :subscription_plan_variant
  delegate :mini?, to: :subscription_plan_variant
  delegate :mini_research?, to: :subscription_plan_variant
  delegate :standard?, to: :subscription_plan_variant
  delegate :standard_research?, to: :subscription_plan_variant
  delegate :pro?, to: :subscription_plan_variant
  delegate :legendary?, to: :subscription_plan_variant
  delegate :research?, to: :subscription_plan_variant
  delegate :research_only?, to: :subscription_plan_variant
  delegate :features, to: :subscription_plan_variant
  delegate :max_bots, to: :subscription_plan_variant

  scope :active, -> { where('ends_at IS NULL OR ends_at > ?', Time.current) }
  scope :by_plan_name, ->(name) { joins(:subscription_plan_variant).merge(SubscriptionPlanVariant.where(subscription_plan: SubscriptionPlan.send(name))) } # rubocop:disable Layout/LineLength

  include Nftable

  before_create :set_ends_at
  after_create_commit :sync_intercom
  after_update_commit :sync_intercom, if: :saved_change_to_ends_at?

  # make subscription immutable, if we need to:
  # - upgrade: create a new subscription with ends_at set to nil || a date in the future
  # - downgrade: modify ends_at to a date in the past
  # - extend duration: modify ends_at to a date in the future
  attr_readonly :user, :subscription_plan_variant

  def active?
    ends_at.nil? || ends_at > Time.current
  end

  def recurring?
    active? && auto_renew?
  end

  def days_left
    return if ends_at.nil?

    [0, (ends_at.to_date - Date.today).to_i].max
  end

  def renews_at
    return ends_at if ends_at.present?
    return nil if subscription_plan_variant.duration.infinite?
    raise 'Subscription duration is zero' if subscription_plan_variant.duration.zero?

    checkpoint = created_at
    loop do
      checkpoint += subscription_plan_variant.duration
      return checkpoint if checkpoint > Time.current
    end
  end

  private

  def set_ends_at
    self.ends_at ||= if subscription_plan_variant.duration == Float::INFINITY
                       nil
                     else
                       subscription_plan_variant.duration.from_now
                     end
  end

  def sync_intercom
    Intercom::UpdateUserSubscriptionJob.perform_later(user)
    return unless ends_at.present?

    Intercom::UpdateUserSubscriptionJob.set(wait_until: ends_at).perform_later(user)
  end
end
