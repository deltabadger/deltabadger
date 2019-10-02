class Subscription < ApplicationRecord
  belongs_to :subscription_plan
  belongs_to :user

  def name
    subscription_plan.name
  end
end
