class UpgradeSubscriptionWorker
  include Sidekiq::Worker

  def perform(user_id, subscription_plan_id, name)
    UpgradeSubscription.call(user_id, subscription_plan_id, name)
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
  end
end
