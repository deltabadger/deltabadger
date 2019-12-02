class User < ApplicationRecord
  after_create :add_subscription
  devise :database_authenticatable, :registerable,
         :recoverable, :rememberable, :validatable, :confirmable

  has_many :api_keys
  has_many :exchanges, through: :api_keys
  has_many :bots
  has_many :subscriptions

  validates :terms_of_service, acceptance: true

  def subscription
    subscriptions.last
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

  def add_subscription
    subscriptions << Subscription.new(
      subscription_plan: SubscriptionPlan.find_by(name: 'free'),
      end_time: created_at + 1.year,
      credits: 1000
    )
  end
end
