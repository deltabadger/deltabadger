class User < ApplicationRecord
  after_create :active_subscription
  devise :database_authenticatable, :registerable,
         :recoverable, :rememberable, :validatable, :confirmable

  has_one :affiliate
  belongs_to :referrer, class_name: 'Affiliate', optional: true
  has_many :api_keys
  has_many :exchanges, through: :api_keys
  has_many :bots
  has_many :subscriptions
  has_many :payments

  validates :terms_and_conditions, acceptance: true
  validate :active_referrer, on: :create

  delegate :unlimited?, to: :subscription

  def subscription
    @subscription ||= active_subscription
  end

  def subscription_name
    subscription.name
  end

  def credits
    subscription.credits
  end

  def first_month?
    month_ago = Date.current - 1.month
    created_at > month_ago
  end

  def limit_reached?
    return false if unlimited?

    credits <= 0
  end

  def welcome_banner_showed?
    welcome_banner_showed
  end

  def eligible_referrer
    referrer if eligible_for_discount?
  end

  private

  def active_subscription
    now = Time.current
    subscriptions.where('end_time > ?', now).order(end_time: :desc).first_or_create do |sub|
      saver_plan = SubscriptionPlansRepository.new.saver
      sub.subscription_plan = saver_plan
      sub.end_time = now + saver_plan.duration
      sub.credits = saver_plan.credits
    end
  end

  def active_referrer
    return if referrer_id.nil? || AffiliatesRepository.new.active?(id: referrer_id)

    errors.add(:referrer, 'code is not valid')
  end

  def eligible_for_discount?
    !payments.paid.where(discounted: true).exists?
  end
end
