class ScheduleTransactionRetry < BaseService
  def initialize(
    make_transaction_worker: MakeTransactionWorker
  )
    @make_transaction_worker = make_transaction_worker
  end

  def call(bot)
    make_transaction_worker.perform_at(
      1.hour,
      bot.id
    )
  end

  private

  attr_reader :make_transaction_worker
end
