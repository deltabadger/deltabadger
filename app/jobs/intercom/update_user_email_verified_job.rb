class Intercom::UpdateUserEmailVerifiedJob < ApplicationJob
  queue_as :default

  def perform(user)
    user.update_intercom_email_verified
  end
end
