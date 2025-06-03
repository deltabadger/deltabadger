class Sendgrid::AddToListJob < ApplicationJob
  queue_as :default

  def perform(user, list_name)
    result = user.add_to_sendgrid_list(list_name)
    raise result.errors.to_sentence if result.failure?
  end
end
