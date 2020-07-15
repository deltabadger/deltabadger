class User < ApplicationRecord
  FREE_SUBSCRIPTION_YEAR_CREDITS_LIMIT =
    ENV.fetch('FREE_SUBSCRIPTION_YEAR_CREDITS_LIMIT')

  after_create :add_subscription
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
    current_subscription = subscriptions.last

    if current_subscription.nil? || current_subscription.end_time < Time.now
      add_subscription
      subscription
    else
      current_subscription
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

  private

  def active_referrer
    return if referrer_id.nil?

    errors.add(:referrer, 'code is not valid') if Affiliate.active.where(id: referrer_id).empty?
  end

  def add_subscription
    subscriptions << Subscription.new(
      subscription_plan: SubscriptionPlan.find_or_create_by(name: 'free'),
      end_time: created_at + 1.year,
      credits: FREE_SUBSCRIPTION_YEAR_CREDITS_LIMIT
    )
  end
end
