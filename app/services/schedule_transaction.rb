class ScheduleTransaction < BaseService
  def initialize(
    bots_repository: BotsRepository.new,
    make_transaction_worker: MakeTransactionWorker,
    parse_interval: ParseInterval.new,
    next_bot_transaction_at: NextBotTransactionAt.new
  )
    @bots_repository = bots_repository
    @make_transaction_worker = make_transaction_worker
    @parse_interval = parse_interval
    @next_bot_transaction_at = next_bot_transaction_at
  end

  def call(bot)
    decrement_bot_interval(bot, last_transaction_at, next_transaction_at)

    next_transaction_at = next_bot_transaction_at.call(bot)
    make_transaction_worker.perform_at(
      next_transaction_at,
      bot.id
    )
  end

  private

  def decrement_bot_interval(bot)
    return unless bot.restarts.zero? && bot.delay.positive?

    interval = parse_interval.call(bot)
    bots_repository.update(bot.id, delay: [bot.delay - interval, 0].max)
  end

  attr_reader :bots_repository, :make_transaction_worker, :parse_interval, :next_bot_transaction_at
end
