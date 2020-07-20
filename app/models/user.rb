class User < ApplicationRecord
  FREE_SUBSCRIPTION_YEAR_CREDITS_LIMIT =
    ENV.fetch('FREE_SUBSCRIPTION_YEAR_CREDITS_LIMIT')

  after_create :subscription
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

  def subscription
    now = Time.current
    subscriptions.where('end_time > ?', now).order(end_time: :desc).first_or_create do |sub|
      sub.subscription_plan = SubscriptionPlan.find_or_create_by(name: 'free')
      sub.end_time = now + 1.year
      sub.credits = FREE_SUBSCRIPTION_YEAR_CREDITS_LIMIT
    end
  end

  def subscription_name
    subscription.name
  end

  def credits
    subscription.credits
  end

  def limit_reached?
    return false if unlimited?

    credits <= 0
  end

  def welcome_banner_showed?
    welcome_banner_showed
  end

  def unlimited?
    subscription_name == 'unlimited'
  end

  def eligible_referrer
    referrer if eligible_for_discount?
  end

  private

  def active_referrer
    return if referrer_id.nil?

    errors.add(:referrer, 'code is not valid') if Affiliate.active.where(id: referrer_id).empty?
  end

  def eligible_for_discount?
    !payments.paid.where(discounted: true).exists?
  end
end
