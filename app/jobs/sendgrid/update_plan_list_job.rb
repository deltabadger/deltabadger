class Sendgrid::UpdatePlanListJob < ApplicationJob
  queue_as :default

  def perform(user)
    subscription = user.subscription
    plan_name_was = user.subscriptions.where.not(id: subscription.id)
                        .where('ends_at IS NULL OR ends_at > ?', subscription.created_at)
                        .order(created_at: :asc).last.name
    plan_name_now = subscription.name
    result = user.change_sendgrid_plan_list(plan_name_was, plan_name_now)
    raise StandardError, result.errors if result.failure?
  end
end
