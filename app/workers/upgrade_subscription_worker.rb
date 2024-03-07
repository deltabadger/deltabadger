class UpgradeSubscriptionWorker
  include Sidekiq::Worker

  def perform(payment, email_params = nil)
    PaymentsManager::SubscriptionUpgrader.call(payment, email_params)
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
  end
end
