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
    credits <= 0
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
