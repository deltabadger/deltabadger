class MakeTransactionWorker
  include Sidekiq::Worker

  def perform(bot_id, continue_params = nil)
    MakeTransaction.call(bot_id, continue_params: continue_params)
    bot = Bot.find(bot_id)
    bot.broadcast_status_bar_update
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
    bot = Bot.find(bot_id)
    bot.broadcast_status_bar_update
  end
end
