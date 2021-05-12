class UpgradeSubscriptionWorker
  include Sidekiq::Worker

  def perform(user_id, subscription_plan_id)
    UpgradeSubscription.call(user_id, subscription_plan_id)
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
  end
end
