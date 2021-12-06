class MakeWithdrawalWorker
  include Sidekiq::Worker

  def perform(bot_id)
    MakeWithdrawal.call(bot_id)
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
  end
end
