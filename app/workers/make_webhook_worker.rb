class MakeWebhookWorker
  include Sidekiq::Worker

  def perform(bot_id, webhook)
    MakeWebhook.call(bot_id, webhook)
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
  end
end
