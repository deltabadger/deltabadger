class MakeTransactionWorker
  include Sidekiq::Worker

  def perform(bot_id, continue_params = nil)
    MakeTransaction.call(bot_id, continue_params: continue_params)
    bot = Bot.find(bot_id)
    Bot::BroadcastStatusBarUpdateJob.perform_later(bot, condition: 'next_action_job_at.present?')
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
    bot = Bot.find(bot_id)
    Bot::BroadcastStatusBarUpdateJob.perform_later(bot, condition: 'next_action_job_at.present?')
  end
end
