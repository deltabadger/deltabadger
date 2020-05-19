class ScheduleTransaction < BaseService
  def initialize(
    make_transaction_worker: MakeTransactionWorker,
    parse_interval: ParseInterval.new
  )

    @make_transaction_worker = make_transaction_worker
    @parse_interval = parse_interval
  end

  def call(bot)
    interval = @parse_interval.call(bot)
    bot.reload
    @make_transaction_worker.perform_at(
      interval.since(bot.last_transaction.created_at).to_i,
      bot.id
    )
  end
end
