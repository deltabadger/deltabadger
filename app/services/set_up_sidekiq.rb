class SetUpSidekiq
  def initialize(
    schedule_transaction: ScheduleTransaction.new
  )
    @schedule_transaction = schedule_transaction
  end

  def fill_sidekiq_queue
    Bot.working.each do |bot|
      @schedule_transaction.call(bot)
    end

    true
  end

  private

  def working?(bot)
    bot.status == 'working'
  end
end
