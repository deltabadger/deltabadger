class MakeTransactionWorker
  include Sidekiq::Worker

  def perform(bot_id, continue_params = nil)
    MakeTransaction.call(bot_id, continue_params: continue_params)
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
  end
end
