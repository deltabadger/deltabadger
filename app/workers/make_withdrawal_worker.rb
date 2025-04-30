class MakeWithdrawalWorker
  include Sidekiq::Worker

  def perform(bot_id)
    MakeWithdrawal.call(bot_id)
    bot = Bot.find(bot_id)
    #  Schedule the broadcast status bar update to make sure sidekiq has time to schedule the job
    Bot::BroadcastStatusBarUpdateJob.set(wait: 0.25.seconds).perform_later(bot)
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
    bot = Bot.find(bot_id)
    #  Schedule the broadcast status bar update to make sure sidekiq has time to schedule the job
    Bot::BroadcastStatusBarUpdateJob.set(wait: 0.25.seconds).perform_later(bot)
  end
end
