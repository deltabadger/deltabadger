class SetUpSidekiq
  def initialize(
    schedule_transaction: ScheduleTransaction.new
  )
    @schedule_transaction = schedule_transaction
  end

  def fill_sidekiq_queue
    Bot.all.each do |bot|
      if working?(bot)
        @schedule_transaction.call(bot)
      end
    end

    true
  rescue StandardError => e
    false
  end

  private

  def working?(bot)
    bot.status == 'working'
  end
end
