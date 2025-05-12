class Sendgrid::SyncPlanListJob < ApplicationJob
  queue_as :default

  def perform(user)
    result = user.sync_sendgrid_plan_list
    raise StandardError, result.errors if result.failure?
  end
end
