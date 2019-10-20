class User < ApplicationRecord
  devise :database_authenticatable, :registerable,
         :recoverable, :rememberable, :validatable, :confirmable

  has_many :api_keys
  has_many :exchanges, through: :api_keys
  has_many :bots
  has_many :subscriptions

  validates :terms_of_service, acceptance: true

  def subscription
    last_subscription = subscriptions.last
    if last_subscription.nil? ||
       (last_subscription && last_subscription.end_time < Time.now)

      return 'free'
    end

    last_subscription.name
  end
end
