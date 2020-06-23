class ScheduleTransactionRestart < BaseService
  def initialize(
    make_transaction_worker: MakeTransactionWorker,
    calculate_restart_delay: CalculateRestartDelay.new
  )
    @make_transaction_worker = make_transaction_worker
    @calculate_restart_delay = calculate_restart_delay
  end

  def call(bot)
    make_transaction_worker.perform_at(
      calculate_restart_delay.call(bot.restarts),
      bot.id
    )
  end

  private

  attr_reader :make_transaction_worker, :calculate_restart_delay
end
