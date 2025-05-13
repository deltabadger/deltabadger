class Sendgrid::UpdateFirstNameJob < ApplicationJob
  queue_as :default

  def perform(user)
    result = user.update_sendgrid_first_name
    raise StandardError, result.errors if result.failure?
  end
end
