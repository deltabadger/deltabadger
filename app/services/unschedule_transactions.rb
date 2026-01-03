class UnscheduleTransactions < BaseService
  def call(bot)
    queue = Sidekiq::ScheduledSet.new
    queue.each do |job|
      job.delete if delete?(job, bot)
    end
  end

  private

  def delete?(job, bot)
    job.args[0] == bot.id && %w[MakeTransactionWorker MakeWithdrawalWorker].include?(job.klass)
  end
end
