class MakeTransactionWorker
  include Sidekiq::Worker

  def perform(bot_id)
    MakeTransaction.call(bot_id)
  end
end
