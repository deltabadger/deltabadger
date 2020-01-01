class Subscription < ApplicationRecord
  belongs_to :subscription_plan
  belongs_to :user

  scope :current, -> { where('end_time > ?', Time.now) }

  def name
    subscription_plan.name
  end
end
