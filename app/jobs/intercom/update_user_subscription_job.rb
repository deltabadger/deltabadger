class Intercom::UpdateUserSubscriptionJob < ApplicationJob
  queue_as :default

  def perform(user)
    user.update_intercom_subscription
  end
end
