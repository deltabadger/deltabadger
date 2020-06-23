class ScheduleTransaction < BaseService
  def initialize(
    bots_repository: BotsRepository.new,
    make_transaction_worker: MakeTransactionWorker,
    parse_interval: ParseInterval.new
  )
    @bots_repository = bots_repository
    @make_transaction_worker = make_transaction_worker
    @parse_interval = parse_interval
  end

  def call(bot)
    interval = parse_interval.call(bot)
    bots_repository.update(bot.id, delay: [bot.delay - interval, 0].max) if bot.delay != 0
    bot.reload
    make_transaction_worker.perform_at(
      interval.since(bot.last_transaction.created_at).to_i,
      bot.id
    )
  end

  private

  attr_reader :bots_repository, :make_transaction_worker, :parse_interval
end
