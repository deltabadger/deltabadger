class ScheduleTransaction < BaseService
  STARTING_BOTS_QUEUE = 'starting bots'.freeze
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

  def call(bot, first_transaction: false, continue_params: nil)
    if bot.restarts.zero? && bot.delay.positive?
      bot = decrease_delay(bot)
    elsif bot.restarts.positive?
      bot = increase_delay(bot)
    end

    next_transaction_at = next_bot_transaction_at.call(bot, first_transaction: first_transaction)
    queue_name = get_queue_name(bot, first_transaction)
    make_transaction_worker.sidekiq_options(queue: queue_name)
    make_transaction_worker.perform_at(
      next_transaction_at,
      bot.id,
      continue_params.to_h
    )
  end

  private

  def get_queue_name(bot, first_transaction)
    exchange_name = bot.exchange.name.downcase
    first_transaction ? STARTING_BOTS_QUEUE : exchange_name
  end

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
