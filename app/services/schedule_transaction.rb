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
    if bot.restarts.zero? && bot.delay.positive?
      bot = decrease_delay(bot)
    elsif bot.restarts.positive?
      bot = increase_delay(bot)
    end

    next_transaction_at = next_bot_transaction_at.call(bot)
    make_transaction_worker.perform_at(
      next_transaction_at,
      bot.id
    )
  end

  private

  def decrease_delay(bot)
    interval = parse_interval.call(bot).to_i
    new_delay = [bot.delay - bot.current_delay, 0].max
    new_current_delay = [new_delay, interval].min
    bots_repository.update(bot.id, delay: new_delay, current_delay: new_current_delay)
  end

  def increase_delay(bot)
    last_transaction_at = bot.last_transaction.created_at
    next_transaction_at = next_bot_transaction_at.call(bot)
    new_delay = (next_transaction_at - last_transaction_at).to_i
    bots_repository.update(bot.id, delay: bot.delay + new_delay)
  end

  attr_reader :bots_repository, :make_transaction_worker, :parse_interval, :next_bot_transaction_at
end
