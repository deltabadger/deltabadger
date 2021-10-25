class ScheduleWithdrawal < BaseService
  STARTING_BOTS_QUEUE = 'starting bots'.freeze
  def initialize(
    make_withdrawal_worker: MakeWithdrawalWorker,
    next_withdrawal_at: NextWithdrawalBotTransactionAt
  )
    @make_withdrawal_worker = make_withdrawal_worker
    @next_withdrawal_at = next_withdrawal_at
  end

  def call(bot, first_transaction: false)
    queue_name = get_queue_name(bot, first_transaction)
    @make_withdrawal_worker.sidekiq_options(queue: queue_name)
    @make_withdrawal_worker.perform_at(
      @next_withdrawal_at.call(bot, first_transaction: first_transaction),
      bot.id
    )
  end

  private

  def get_queue_name(bot, first_transaction)
    exchange_name = bot.exchange.name.downcase
    first_transaction ? STARTING_BOTS_QUEUE : exchange_name
  end

  attr_reader :make_withdrawal_worker
end