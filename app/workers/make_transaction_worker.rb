class MakeTransactionWorker
  include Sidekiq::Worker

  def perform(bot_id, continue_params = nil)
    MakeTransaction.call(bot_id, continue_params: continue_params)
    bot = Bot.find(bot_id)
    Bot::BroadcastAfterScheduledActionJob.perform_later(bot)
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
    bot = Bot.find(bot_id)
    Bot::BroadcastAfterScheduledActionJob.perform_later(bot)
  end
end
