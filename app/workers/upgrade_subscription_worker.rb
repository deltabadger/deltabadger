class UpgradeSubscriptionWorker
  include Sidekiq::Worker

  def perform(user_id, subscription_plan_id, email_params, payment_id = nil)
    UpgradeSubscription.call(user_id, subscription_plan_id, email_params, payment_id)
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
  end
end
