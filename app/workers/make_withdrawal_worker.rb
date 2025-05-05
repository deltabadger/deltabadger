class MakeWithdrawalWorker
  include Sidekiq::Worker

  def perform(bot_id)
    MakeWithdrawal.call(bot_id)
    bot = Bot.find(bot_id)
    bot.broadcast_status_bar_update
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
    bot = Bot.find(bot_id)
    bot.broadcast_status_bar_update
  end
end
