class MakeWithdrawalWorker
  include Sidekiq::Worker

  def perform(bot_id)
    MakeWithdrawal.call(bot_id)
    bot = Bot.find(bot_id)
    Bot::BroadcastStatusBarUpdateJob.perform_later(bot, condition: 'next_action_job_at.present?')
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
    bot = Bot.find(bot_id)
    Bot::BroadcastStatusBarUpdateJob.perform_later(bot, condition: 'next_action_job_at.present?')
  end
end
