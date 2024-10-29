class UpgradeSubscriptionWorker
  include Sidekiq::Worker

  # worker needs params that serialize to json
  def perform(payment_id)
    payment = Payment.find(payment_id)
    PaymentsManager::SubscriptionUpgrader.call(payment)
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
  end
end
