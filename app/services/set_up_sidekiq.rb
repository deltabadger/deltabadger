class SetUpSidekiq
  def initialize(
    schedule_transaction: ScheduleTransaction.new,
    schedule_withdrawal: ScheduleWithdrawal.new,
    schedule_webhook: ScheduleWebhook.new
  )
    @schedule_transaction = schedule_transaction
    @schedule_withdrawal = schedule_withdrawal
    @schedule_webhook = schedule_webhook
  end

  def fill_sidekiq_queue
    Bot.working.each do |bot|
      @schedule_transaction.call(bot) if bot.trading?
      @schedule_withdrawal.call(bot) if bot.withdrawal?
      @schedule_webhook.call(bot) if bot.webhook?
    end

    true
  end

  private

  def working?(bot)
    bot.status == 'working'
  end
end
