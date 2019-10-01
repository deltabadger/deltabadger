class User < ApplicationRecord
  devise :database_authenticatable, :registerable,
         :recoverable, :rememberable, :validatable, :confirmable

  has_many :api_keys
  has_many :exchanges, through: :api_keys
  has_many :bots
  has_many :subscriptions

  def subscription
    return 'free' if subscriptions.last.end_time < Time.now

    subscriptions.last.name
  end
end
