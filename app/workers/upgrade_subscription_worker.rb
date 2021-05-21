class UpgradeSubscriptionWorker
  include Sidekiq::Worker

  def perform(user_id, subscription_plan_id, email_params)
    UpgradeSubscription.call(user_id, subscription_plan_id, email_params)
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
  end
end
