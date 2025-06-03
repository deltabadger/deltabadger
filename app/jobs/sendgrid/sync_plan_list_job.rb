class Sendgrid::SyncPlanListJob < ApplicationJob
  queue_as :default

  def perform(user)
    result = user.sync_sendgrid_plan_list
    raise result.errors.to_sentence if result.failure?
  end
end
