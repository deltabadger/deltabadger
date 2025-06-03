class Sendgrid::UpdateFirstNameJob < ApplicationJob
  queue_as :default

  def perform(user)
    result = user.update_sendgrid_first_name
    raise result.errors.to_sentence if result.failure?
  end
end
