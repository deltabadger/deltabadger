class UpgradeSubscriptionWorker
  include Sidekiq::Worker

  def perform(payment)
    PaymentsManager::SubscriptionUpgrader.call(payment)
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
  end
end
