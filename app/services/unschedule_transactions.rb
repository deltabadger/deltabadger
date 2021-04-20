class UnscheduleTransactions < BaseService
  def call(bot)
    queue = Sidekiq::ScheduledSet.new
    queue.each do |job|
      job.delete
    end
  end

  private

  def delete?(job, bot)
    job.args[0] == bot.id &&
      job.klass == 'MakeTransactionWorker'
  end
end
