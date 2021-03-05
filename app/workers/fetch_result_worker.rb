class FetchResultWorker
  include Sidekiq::Worker

  def perform(bot_id, offer_id)
    FetchOrderResult.call(bot_id, offer_id)
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
  end
end
